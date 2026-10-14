#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) validation of the cap-loop fillet fixtures written by
#   RCAD_EVIDENCE_DIR=DIR cargo test -p rcad-kernel --lib fillet::cap
# usage: tools/heavy.sh tools/check_cap_fillet.sh DIR
# Expected volumes are closed forms derived per case (spandrel area A = r^2 (1 - pi/4),
# first moment M = r^3 (5/6 - pi/4) about a wall; Pappus for arcs; miter corners -+M per
# edge); bounds are the unfilleted prism's; probes sit 0.02 mm off the analytic fillet surface.
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_cap_fillet.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: cap fillet STEP validation NOT RUN"; exit 2
fi
status=0
check() { # file, expected volume (python expr), bounds (python list), probes (python expr), expected inside
  local f=$1 vol=$2 bounds=$3 probes=$4 inside=$5 out pj
  pj=$(python3 -c "import json, math; s=math.sqrt(0.5); d=1/math.sqrt(3); print(json.dumps($probes))")
  out=$(timeout 300 flatpak run --env="RCAD_STEP=$DIR/$f" --env="RCAD_PROBES=$pj" \
    --filesystem="$DIR" --filesystem="$REPO" --command=FreeCADCmd org.freecad.FreeCAD \
    "$REPO/tools/freecad_check_probes.py" 2>&1 | grep FREECAD_RESULT) || { echo "$f: FreeCAD failed"; status=1; return; }
  echo "$out" > "$DIR/${f%.step}_freecad.txt"
  python3 - "$f" "$out" "$vol" "$bounds" "$inside" <<'PY' || status=1
import json, math, sys
pi = math.pi
A = lambda r: r * r * (1 - pi / 4)
M = lambda r: r ** 3 * (5 / 6 - pi / 4)
env = {"math": math, "pi": pi, "A": A, "M": M}
f, out = sys.argv[1], sys.argv[2]
exp_v = eval(sys.argv[3], env); exp_b = eval(sys.argv[4], env); exp_in = eval(sys.argv[5], env)
r = json.loads(out.split(" ", 1)[1])
ok = bool(r.get("ok") and r.get("isValid") and r.get("solids") == 1)
g = (r.get("solid_volumes") or [float("nan")])[0]
tol = max(0.001, exp_v * 1e-7); good = abs(g - exp_v) <= tol; ok = ok and good
print(f"  {f}: solids={r.get('solids')} valid={r.get('isValid')} volume {g:.9f} vs {exp_v:.9f} err {abs(g-exp_v):.2e} (tol {tol:.1e}) {'ok' if good else 'BAD'}")
berr = max(abs(a - b) for a, b in zip(r.get("bounds", [0]*6), exp_b)) if r.get("bounds") else 1e9
ok = ok and berr <= 1e-6
print(f"  {f}: bounds {[round(x, 9) for x in r.get('bounds', [])]} max err {berr:.2e}")
ok = ok and r.get("inside") == exp_in
print(f"  {f}: probes inside {r.get('inside')} expected {exp_in}")
print(f"{f}: {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
}
# 40x30x10 box, top loop r = 2 (cylinders, four convex miters).
check cap_fillet_sharp_box.step "12000 - A(2)*140 + 8*M(2)" "[0,0,0,40,30,10]" \
  "[[20, 2-2*s-0.02*s, 8+2*s+0.02*s], [20, 2-2*s+0.02*s, 8+2*s-0.02*s], [20,15,5]]" \
  "[False, True, True]"
# Rounded rectangle 40x30 (R 5) x 12, top loop r = 2 (cylinders + tori).
check cap_fillet_rounded_rect.step \
  "(1200 - (4-pi)*25)*12 - A(2)*(2*30+2*20) - 2*pi*(A(2)*5 - M(2))" "[0,0,0,40,30,12]" \
  "[[5-(3+2*s)*s-0.01, 5-(3+2*s)*s-0.01, 10+2*s+0.02*s], [5-(3+2*s)*s+0.01, 5-(3+2*s)*s+0.01, 10+2*s-0.02*s]]" \
  "[False, True]"
# Rounded rectangle 30x20 (R 3) x 8, top loop r = 3 (sphere corners).
check cap_fillet_sphere_corners.step \
  "(600 - (4-pi)*9)*8 - A(3)*(2*24+2*14) - 2*pi*(A(3)*3 - M(3))" "[0,0,0,30,20,8]" \
  "[[3-3.02*d, 3-3.02*d, 5+3.02*d], [3-2.98*d, 3-2.98*d, 5+2.98*d]]" "[False, True]"
# L plate (40x15 + 15x15 arm) x 6, top loop r = 1.5: five convex, one concave miter.
check cap_fillet_l_plate.step "825*6 - A(1.5)*140 + 8*M(1.5)" "[0,0,0,40,30,6]" \
  "[[20,0.02,5.98],[20,0.5,5.5]]" "[False, True]"
# Box 40x30x10 with vertical edges filleted R 4 (edge fillet), then the top loop r = 4 / 1.5.
for r in 4 1.5; do
  check "cap_fillet_after_edge_fillets_r$r.step" \
    "(1200 - (4-pi)*16)*10 - A($r)*(2*32+2*22) - 2*pi*(A($r)*4 - M($r))" "[0,0,0,40,30,10]" \
    "[[20,15,5],[20,0.02,9.98]]" "[True, False]"
done
exit $status
