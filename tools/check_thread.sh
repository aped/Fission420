#!/usr/bin/env bash
# Independent FreeCAD (OpenCascade) check of the faceted thread exports written by
#   RCAD_EVIDENCE_DIR=DIR cargo test --release -p rcad-kernel --lib thread
# usage: tools/heavy.sh tools/check_thread.sh DIR
# Expects: rod and nut each one valid solid; FreeCAD volumes equal our exact (planar)
# volumes 3215.09586 / 7515.00173 mm^3 to 1e-6 relative; rod ∩ nut empty (< 1e-6 mm^3);
# control: the nut moved half a pitch clashes with the rod (common > 10 mm^3).
set -u -o pipefail
DIR=$(realpath "${1:?usage: check_thread.sh DIR}")
REPO=$(cd "$(dirname "$0")/.." && pwd)
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: thread STEP validation NOT RUN"; exit 2
fi
out=$(timeout 900 flatpak run --env="RCAD_ROD=$DIR/thread_rod_m20x2.step" \
  --env="RCAD_NUT=$DIR/thread_nut_m20x2.step" --env=RCAD_HALF_PITCH=1 \
  --filesystem="$DIR" --filesystem="$REPO" --command=FreeCADCmd org.freecad.FreeCAD \
  "$REPO/tools/freecad_check_thread.py" 2>&1 | grep FREECAD_RESULT) || { echo "FreeCAD failed"; exit 1; }
echo "$out" > "$DIR/freecad_thread.txt"
python3 - "$out" <<'PY'
import json, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
ok = r.get("ok", False)
for k, want in (("rod", 3215.09586), ("nut", 7515.00173)):
    x = r.get(k, {})
    good = x.get("valid") and x.get("solids") == 1 and abs(x.get("volume", 0) - want) <= 1e-6 * want + 1e-5
    print(f"  {k}: valid={x.get('valid')} solids={x.get('solids')} faces={x.get('faces')} volume {x.get('volume')} vs {want} {'ok' if good else 'BAD'}")
    ok = ok and bool(good)
c, ctl = r.get("common", 1e9), r.get("control_common", 0)
print(f"  rod ∩ nut = {c:.3e} mm^3 (want < 1e-6); control (nut moved p/2) = {ctl:.3f} mm^3 (want > 10)")
ok = ok and c < 1e-6 and ctl > 10
print("threads: " + ("PASS" if ok else "FAIL"))
sys.exit(0 if ok else 1)
PY
