#!/usr/bin/env bash
# Independent check of multi-body STEP export (rcad_kernel::export_step_bodies) with
# FreeCAD 1.1 flatpak (OpenCascade), not the Rust writer/reader.
# usage: tools/check_step_multi.sh
# Writes artifacts/step_multi/multi.step via the ignored test
# `step_export_multi::write_multi_step_artifact`, then requires: 3 solids, valid shape,
# and sorted per-solid volumes within max(0.001, V*1e-7) of the analytic values
# (computed here, not read from R-CAD output):
#   Block 20 x 10 x 5:                       1000
#   Pin   Ø10 x 12 cylinder:                  300*pi
#   Plate 30 x 20 x 10, centred Ø8 through:   6000 - 160*pi
set -u -o pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
DIR="$REPO/artifacts/step_multi"
cargo test --release -q -p rcad-kernel --test step_export_multi -- --ignored write_multi_step_artifact || exit 1
if ! flatpak info org.freecad.FreeCAD >/dev/null 2>&1; then
  echo "FreeCAD flatpak not installed: external multi-body STEP validation NOT RUN"
  exit 2
fi
out=$(timeout 180 flatpak run --env="RCAD_STEP=$DIR/multi.step" --env=RCAD_EXPECT_SOLIDS=3 \
  --filesystem="$DIR" --command=FreeCADCmd org.freecad.FreeCAD \
  "$REPO/tools/freecad_check_step.py" 2>&1 | grep FREECAD_RESULT) || exit 1
echo "$out" | tee "$DIR/multi_step_check_freecad.txt"
python3 - "$out" <<'PY'
import json, math, sys
r = json.loads(sys.argv[1].split(" ", 1)[1])
exp = sorted([1000.0, 300 * math.pi, 6000 - 160 * math.pi])
got = r.get("solid_volumes", [])
ok = bool(r.get("ok") and r.get("isValid") and r.get("expect_solids_ok") and len(got) == len(exp))
for g, e in zip(got, exp):
    err = abs(g - e)
    tol = max(0.001, e * 1e-7)
    ok = ok and err <= tol
    print(f"  solid volume {g:.9f} vs analytic {e:.9f}: err={err:.3e} (tol {tol:.1e})")
print(f"multi-body STEP via OpenCascade: solids={r.get('solids')} valid={r.get('isValid')} -> {'PASS' if ok else 'FAIL'}")
sys.exit(0 if ok else 1)
PY
