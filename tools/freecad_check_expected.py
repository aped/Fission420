# Independent STEP check (FreeCAD/OpenCascade) against an expected solid that FreeCAD
# builds itself from Part primitives:
# flatpak run --env=RCAD_STEP=<abs path> --env=RCAD_EXPECT='<python>' --filesystem=<dir> \
#   --command=FreeCADCmd org.freecad.FreeCAD tools/freecad_check_expected.py
# RCAD_EXPECT is Python run with Part, V (= FreeCAD.Vector) and math in scope; it must
# assign the expected shape to `exp`. Prints FREECAD_RESULT {ok, solids, isValid,
# volume, expected_volume, sym_diff}: sym_diff = volume of (read \ exp) + (exp \ read),
# OpenCascade booleans of the two exact models (0 when the read solid is the expected one).
import os, json, math, Part, FreeCAD
s = Part.Shape()
try:
    s.read(os.environ["RCAD_STEP"])
    env = {"Part": Part, "V": FreeCAD.Vector, "math": math}
    exec(os.environ["RCAD_EXPECT"], env)
    exp = env["exp"]
    a = s.cut(exp).Volume
    b = exp.cut(s).Volume
    r = {"ok": True, "solids": len(s.Solids), "isValid": s.isValid(), "volume": s.Volume,
         "expected_volume": exp.Volume, "sym_diff": a + b, "read_minus_exp": a, "exp_minus_read": b}
    print("FREECAD_RESULT " + json.dumps(r))
except Exception as e:
    print("FREECAD_RESULT " + json.dumps({"ok": False, "error": str(e)}))
