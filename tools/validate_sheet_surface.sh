#!/usr/bin/env bash
# External (FreeCAD / OpenCascade) validation of the sheet-metal and surface
# acceptance outputs, written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-commands --test sheet_surface_acceptance
# usage: tools/heavy.sh tools/validate_sheet_surface.sh DIR
# Expected values are closed form (derived here, not read from R-CAD):
#   U-channel, width 40, K 0.4, straights between tangent points 60 + 20 + 20, two 90° bends:
#     folded V = 40 t 100 + 40 · 2 · (π/4)((R+t)² − R²); flat L = 100 + π (R + 0.4 t)
#     folded bbox [−(R+t), 0, 0] .. [60 + R + t, 40, t + R + 20]
#     flat DXF: CUT one closed wire, area 40 L, x extent L (from −(BA+20)), BEND_UP 2 lines of 40
#   open box (no gap) filled: V = 24000, bbox [0,0,0]..[40,30,20]
#   gap box (0.02 mm gap healed at 0.05): |V − 24000| ≤ 0.02 · 600
#   half tube r 10, z 0..30, thickened 2 mm outward: V = (π/2)(12² − 10²) 30
set -u -o pipefail
DIR=$(realpath "${1:?usage: validate_sheet_surface.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: validation NOT RUN"; exit 2
fi
status=0
fc() { # script env-name file
  timeout 300 flatpak run --env="$2=$DIR/$3" --filesystem="$DIR" --filesystem="$REPO" \
    --command=FreeCADCmd org.freecad.FreeCAD "$REPO/tools/$1" 2>&1 | grep -a -o "FREECAD_RESULT.*"
}
step() { # file expected-volume-expr tolerance bbox-expr-or-None
  local out; out=$(fc freecad_check_step.py RCAD_STEP "$1") || { echo "$1: FreeCAD failed"; status=1; return; }
  echo "$out" > "$DIR/${1%.step}_freecad.txt"
  python3 -I - "$1" "$out" "$2" "$3" "$4" <<'PY' || status=1
import json, math, sys
pi = math.pi
f, out, ve, tol, be = sys.argv[1:6]
r = json.loads(out.split(" ", 1)[1])
v, t = eval(ve), eval(tol)
ok = r.get("ok") and r.get("isValid") and r.get("solids") == 1
g = (r.get("solid_volumes") or [float("nan")])[0]
good = abs(g - v) <= t and r.get("volume", -1) > 0  # positive: outward orientation
msg = f"{f}: solids={r.get('solids')} valid={r.get('isValid')} V {g:.9f} vs {v:.9f} (tol {t:g})"
if be != "None":
    b = eval(be); bb = r.get("bounds", [])
    berr = max(abs(x - y) for x, y in zip(bb, b))
    good = good and berr <= 1e-6
    msg += f" bbox err {berr:.2e}"
ok = ok and good
print(msg, "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
PY
}
flat() { # file t R
  local out; out=$(fc freecad_check_flat_dxf.py RCAD_DXF "$1") || { echo "$1: FreeCAD failed"; status=1; return; }
  echo "$out" > "$DIR/${1%.dxf}_freecad.txt"
  python3 -I - "$1" "$out" "$2" "$3" <<'PY' || status=1
import json, math, sys
f, out, t, R = sys.argv[1], sys.argv[2], float(sys.argv[3]), float(sys.argv[4])
r = json.loads(out.split(" ", 1)[1])
L = 100 + math.pi * (R + 0.4 * t); ba = math.pi / 2 * (R + 0.4 * t)
c, b = r["layers"]["CUT"], r["layers"].get("BEND_UP", {})
checks = {
  "insunits mm": r["insunits"] == "4",
  "cut closed single wire": c.get("wires") == 1 and c.get("closed"),
  "cut area": abs(r["cut_area"] - 40 * L) < 1e-6,
  "cut x extent": abs(c["xmin"] + 20 + ba) < 1e-6 and abs(c["xmax"] - 80 - ba) < 1e-6,
  "bend up lines": b.get("lines") == 2 and abs(b.get("length", 0) - 80) < 1e-6,
  "bend lines at BA/2": abs(b.get("xmin", 0) + ba / 2) < 1e-6 and abs(b.get("xmax", 0) - 60 - ba / 2) < 1e-6,
  "no bend down": "BEND_DOWN" not in r["layers"],
}
ok = all(checks.values())
print(f"{f}: flat L {L:.6f}, area {r['cut_area']:.6f} vs {40*L:.6f}", {k: v for k, v in checks.items() if not v} or "all checks", "PASS" if ok else "FAIL")
sys.exit(0 if ok else 1)
PY
}
for tr in "1 2" "2 3"; do
  set -- $tr; t=$1; R=$2; tag="t${t}_r${R}"
  step "u_channel_${tag}_folded.step" "40*$t*100 + 80*(pi/4)*(($R+$t)**2 - $R**2)" 1e-6 "[-($R+$t), 0, 0, 60+$R+$t, 40, $t+$R+20]"
  step "u_channel_${tag}_flat.step" "40*$t*(100 + pi*($R+0.4*$t))" 1e-6 "[-(20+pi/2*($R+0.4*$t)), 0, 0, 80+pi/2*($R+0.4*$t), 40, $t]"
  flat "u_channel_${tag}_flat.dxf" "$t" "$R"
done
step open_box_filled.step 24000 1e-6 "[0, 0, 0, 40, 30, 20]"
step gap_box_filled.step 24000 "0.02*600 + 1e-6" None
step half_tube_thick.step "pi/2*(144-100)*30" 1e-6 "[0, -12, 0, 12, 12, 30]"
# Open hem (180°, R 0.5, t 1, leg 10 back over a 60 x 40 plate): 2400 + 60 (π/2)(1.5² − 0.5²) + 600
step hem_plate.step "3000 + 60*pi" 1e-6 "[0, -1.5, 0, 60, 40, 3]"
echo "overall: $([ $status -eq 0 ] && echo PASS || echo FAIL)"
exit $status
