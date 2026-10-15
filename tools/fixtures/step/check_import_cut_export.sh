#!/usr/bin/env bash
# Independent check of the STEP import -> edit -> export path with FreeCAD 1.1 (OpenCascade).
# usage: tools/fixtures/step/check_import_cut_export.sh
# The test `step_import::imported_box_can_be_cut_measured_and_exported` imports the FreeCAD
# box 40x30x10, cuts a centred Ø6 through hole from its imported top-face frame and writes
# target/tmp/imported_box_hole.step with export_step. FreeCAD must read 1 valid solid with
# volume 12000 - 90*pi (analytic, computed here) within max(0.001, V*1e-7).
set -u -o pipefail
REPO=$(cd "$(dirname "$0")/../../.." && pwd)
cargo test --release -q -p rcad-kernel --test it step_import::imported_box_can_be_cut_measured_and_exported || exit 1
FILE="$REPO/target/tmp/imported_box_hole.step"
mkdir -p "$REPO/artifacts/step_import"
cp "$FILE" "$REPO/artifacts/step_import/imported_box_hole.step"
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: external validation NOT RUN"
  exit 2
fi
out=$(timeout 180 flatpak run --env="RCAD_STEP=$REPO/artifacts/step_import/imported_box_hole.step" \
  --env=RCAD_EXPECT_SOLIDS=1 --filesystem="$REPO/artifacts/step_import" \
  --command=FreeCADCmd org.freecad.FreeCAD "$REPO/tools/freecad_check_step.py" 2>&1 | grep FREECAD_RESULT) || exit 1
echo "$out" | tee "$REPO/artifacts/step_import/imported_box_hole_freecad.txt"
python3 - "$out" <<'PY'
import json, math, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
exp = 12000 - 90 * math.pi
err = abs(r.get("volume", 0) - exp)
ok = r.get("ok") and r.get("isValid") and r.get("solids") == 1 and err <= max(0.001, exp * 1e-7)
print(f"imported box + Ø6 hole via OpenCascade: solids={r.get('solids')} valid={r.get('isValid')} volume_err={err:.3e} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
