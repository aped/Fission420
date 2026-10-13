# Independent check of the faceted thread exports (FreeCAD/OpenCascade):
# flatpak run --env=RCAD_ROD=<abs> --env=RCAD_NUT=<abs> --filesystem=<dir> \
#   --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_thread.py
# Prints FREECAD_RESULT {ok, rod: {valid, solids, volume, bounds}, nut: {...}, common}:
# `common` is the volume of rod ∩ nut computed by OpenCascade (0 when they mate);
# `control_common` the same with the nut moved half a pitch (RCAD_HALF_PITCH, mm) along
# the axis: the flanks then clash, so it must be clearly positive (a silently failing
# boolean cannot pass).
import os, json, Part
import FreeCAD as App


def info(path):
    s = Part.Shape()
    s.read(path)
    bb = s.optimalBoundingBox()
    return s, {"valid": s.isValid(), "solids": len(s.Solids), "faces": len(s.Faces),
               "volume": s.Volume, "bounds": [bb.XMin, bb.YMin, bb.ZMin, bb.XMax, bb.YMax, bb.ZMax]}


try:
    rod, ri = info(os.environ["RCAD_ROD"])
    nut, ni = info(os.environ["RCAD_NUT"])
    common = rod.common(nut).Volume
    moved = nut.copy()
    moved.translate(App.Vector(0, 0, float(os.environ.get("RCAD_HALF_PITCH", "1"))))
    control = rod.common(moved).Volume
    print("FREECAD_RESULT " + json.dumps({"ok": True, "rod": ri, "nut": ni, "common": common,
                                          "control_common": control}))
except Exception as e:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": str(e)}))
