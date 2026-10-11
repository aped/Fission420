#!/usr/bin/env bash
# Regenerate the STEP fixtures in crates/f420-kernel/tests/fixtures/step/.
#
# Needs the FreeCAD flatpak (org.freecad.FreeCAD; fixtures were made with
# 1.1.4 / OCC 7.8.1) and python3. Host flatpak permissions are not changed:
# the repo is exposed for these invocations only via --filesystem.
#
# Steps:
#   1. generate.py (FreeCADCmd, F420_MODE=generate): primitive fixtures.
#   2. make_units.py (python3): box_40x30x10_{inch,m}.step, scaled text
#      conversions of box_40x30x10_mm.step (FreeCAD only relabels units).
#      make_derived.py (python3): truncated / garbage / missing_unit
#      text derivations of box_40x30x10_mm.step.
#   3. generate.py (FreeCADCmd, F420_MODE=verify): re-import every .step
#      and write verification.json next to the fixtures.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
OUT="$REPO/crates/f420-kernel/tests/fixtures/step"
mkdir -p "$OUT"

freecad() {
    flatpak run --filesystem="$REPO" \
        --env=F420_OUT="$OUT" --env=F420_RESULTS="$OUT/verification.json" \
        --env=F420_MODE="$1" \
        --command=FreeCADCmd org.freecad.FreeCAD "$HERE/generate.py"
}

freecad generate
python3 "$HERE/make_units.py" "$OUT"
python3 "$HERE/make_derived.py" "$OUT"
freecad verify
echo "fixtures written to $OUT"
