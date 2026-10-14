#!/usr/bin/env bash
# Independent check of STEP export of ellipse / spline profiles (exact rational and
# polynomial B-spline edges and their extruded surfaces) with FreeCAD 1.1 flatpak
# (OpenCascade), not the Rust writer/reader.
# usage: tools/check_step_curves.sh
# Writes artifacts/step_curves/*.step via the ignored test
# `ellipse_spline::write_curve_step_artifacts`, then requires for each file: 1 valid
# solid and a volume within max(0.001, V*1e-7) of the analytic value (computed here):
#   ellipse_prism:      ellipse a 20, b 10, height 15:            pi*20*10*15
#   ellipse_cut_block:  60 x 40 x 10 block minus ellipse 12 x 6:   24000 - pi*12*6*10
#   spline_d_prism:     cubic span (30,0)(30,20)(0,20)(0,0) closed by the x axis,
#                       area (3/5)*20*30, height 5:                0.6*20*30*5
set -u -o pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
DIR="$REPO/artifacts/step_curves"
cargo test --release -q -p rcad-kernel --test ellipse_spline -- --ignored write_curve_step_artifacts || exit 1
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: external curve STEP validation NOT RUN"
  exit 2
fi
fail=0
for spec in "ellipse_prism:pi*20*10*15" "ellipse_cut_block:24000-pi*12*6*10" "spline_d_prism:0.6*20*30*5"; do
  name=${spec%%:*}
  expr=${spec#*:}
  out=$(timeout 180 flatpak run --env="RCAD_STEP=$DIR/$name.step" --env=RCAD_EXPECT_SOLIDS=1 \
    --filesystem="$DIR" --command=FreeCADCmd org.freecad.FreeCAD \
    "$REPO/tools/freecad_check_step.py" 2>&1 | grep FREECAD_RESULT) || { echo "$name: no FreeCAD result"; fail=1; continue; }
  echo "$name $out" >> "$DIR/check_freecad.txt"
  python3 - "$out" "$expr" "$name" <<'PY' || fail=1
import json, sys
from math import pi
r = json.loads(sys.argv[1].split(" ", 1)[1])
exp = eval(sys.argv[2], {"pi": pi})
v = r.get("volume", float("nan"))
tol = max(0.001, exp * 1e-7)
ok = bool(r.get("ok") and r.get("isValid") and r.get("expect_solids_ok") and abs(v - exp) <= tol)
print(f"{sys.argv[3]}: volume {v:.9f} vs analytic {exp:.9f} err={abs(v-exp):.3e} (tol {tol:.1e}) "
      f"surfaces={r.get('surfaces')} valid={r.get('isValid')} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
done
exit $fail
