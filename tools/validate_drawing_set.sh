#!/usr/bin/env bash
# Drawing set acceptance: build the holed plate and its pinned assembly with
# tools/fixtures/drawing_set_workflow.json, then check the exported sheets with independent
# readers: poppler (pdfinfo/pdftotext/pdftoppm) for the PDFs and FreeCAD's DXF importer
# (flatpak org.freecad.FreeCAD, tools/freecad_check_drawing_dxf.py) for the part DXF.
# Expected values are hand-derived from the workflow:
#   plate 80 x 50 x 10, Ø8 at (20,25), Ø6 at (60,25); front/top/right at 1:2, section A-A
#   at x = 20 from the right, detail B (2:1) of the Ø6 hole.
#   texts: "10 ±0.1" (thickness), "40 +0.05/-0.05" (hole spacing), "Ø8.1/8" (limits),
#   after the edit (Ø10 at x = 22): "38 +0.05/-0.05", "Ø10.1/10"; after deleting the Ø6
#   hole: the spacing dimension is dangling ("? distance") and "38 +0.05/-0.05" is gone.
#   DXF (part, 1:2): HIDDEN = 4 hole silhouettes in the front view + 4 in the right view,
#   each 10 x 0.5 = 5 mm, total 40 mm; VISIBLE circles: Ø8 and Ø6 in the top view + the
#   Ø6 in detail B = 3. CENTER: marks 62 mm + cutting line 17 mm + hole centre lines seen
#   side-on (9 mm each: front 2, right 1 merged, section 1) = 11 lines, 115 mm.
#   assembly sheet: BOM rows "1 PL-80 Plate 1 PLA" and "2 PIN-6 Pin 2 -", balloons 1 and 2.
# Usage: tools/validate_drawing_set.sh <rcad-cli> [out-dir]   (run from the repo root)
set -euo pipefail
CLI=$(readlink -f "${1:?usage: validate_drawing_set.sh <rcad-cli> [out-dir]}")
OUT=${2:-artifacts/drawing_set}
ROOT=$(pwd)
mkdir -p "$OUT"
# The workflow writes to artifacts/drawing_set; copy results to OUT if different.
SRC=artifacts/drawing_set
mkdir -p "$SRC"   # the workflow writes there (absent in a clean checkout)
"$CLI" run tools/fixtures/drawing_set_workflow.json > "$OUT/run.jsonl" ||
  { echo "FAILED: workflow (see $OUT/run.jsonl)"; exit 1; }
[ "$(readlink -f "$SRC")" = "$(readlink -f "$OUT")" ] || cp "$SRC"/*.{pdf,dxf,svg} "$SRC"/drawing_set.rcad.json "$OUT"/
fail=0
check() { if "$@"; then echo "PASS: ${*: -1}"; else echo "FAIL: ${*: -1}"; fail=1; fi; }
has() { grep -qF -- "$2" <<<"$1"; }
for f in part assembly part_edited part_hole_deleted; do
  pdfinfo "$OUT/$f.pdf" | grep -q "Page size: *841.89 x 595.276 pts (A4)" && echo "PASS: $f A4" || { echo "FAIL: $f page size"; fail=1; }
done
P=$(pdftotext -layout "$OUT/part.pdf" -)
for t in "10 ±0.1" "40 +0.05/-0.05" "Ø8.1/8" "SECTION A-A" "DETAIL B (2:1)" "PL-80" "PLA" \
         "THIRD ANGLE PROJECTION" "1. BREAK SHARP EDGES 0.2"; do
  check has "$P" "$t"
done
E=$(pdftotext -layout "$OUT/part_edited.pdf" -)
for t in "38 +0.05/-0.05" "Ø10.1/10" "10 ±0.1"; do check has "$E" "$t"; done
D=$(pdftotext -layout "$OUT/part_hole_deleted.pdf" -)
check has "$D" "? distance"
if has "$D" "38 +0.05/-0.05"; then echo "FAIL: stale spacing value drawn"; fail=1; else echo "PASS: no stale spacing value"; fi
A=$(pdftotext -layout "$OUT/assembly.pdf" -)
for t in "ITEM" "PIN-6" "PL-80" "QTY" "Pinned plate" "ASM-1"; do check has "$A" "$t"; done
check grep -qE "^ *2 +PIN-6 +Pin +2" <<<"$A"
check grep -qE "^ *1 +PL-80 +Plate +1 +PLA" <<<"$A"
for f in part assembly part_edited part_hole_deleted; do
  pdftoppm -r 110 -png "$OUT/$f.pdf" "$OUT/$f"
done
echo "rendered: $(ls "$OUT"/*.png | tr '\n' ' ')"
if command -v flatpak >/dev/null; then
  RES=$(tools/heavy.sh flatpak run --env=RCAD_DXF="$(readlink -f "$OUT/part.dxf")" --filesystem="$ROOT" \
    --filesystem="$(readlink -f "$OUT")" --command=FreeCADCmd org.freecad.FreeCAD \
    "$ROOT/tools/freecad_check_drawing_dxf.py" 2>&1 | tee "$OUT/freecad_dxf.log" | grep -a -o "FREECAD_RESULT.*")
  echo "$RES"
  python3 -I - "$RES" <<'PY' || fail=1
import json, sys
r = json.loads(sys.argv[1].split("FREECAD_RESULT ", 1)[1])
L = r["layers"]
checks = {
    "dxf read": r["ok"],
    "hidden 8 lines": L["HIDDEN"]["lines"] == 8,
    "hidden 40 mm": abs(L["HIDDEN"]["line_length"] - 40.0) < 1e-6,
    "3 visible circles": L["VISIBLE"]["circles"] == 3,
    # Centre marks: top view Ø8 2 x 8 + Ø6 2 x 7 = 30 mm, detail B (2:1) 2 x 16 = 32 mm;
    # cutting line 17 mm; hole centre lines seen side-on (2 sheet mm past the 10 mm plate
    # at 1:2 = 9 mm each): front x 2, right x 1 (both holes project onto one line),
    # section A-A x 1 (hole A's axis lies on the cutting plane) = 36 mm. 11 lines, 115 mm.
    "centre layer 11 lines 115 mm": L["CENTER"]["lines"] == 11
        and abs(L["CENTER"]["line_length"] - 115.0) < 1e-6,
}
for k, v in checks.items():
    print(("PASS: " if v else "FAIL: ") + "FreeCAD " + k)
sys.exit(0 if all(checks.values()) else 1)
PY
  if grep -aiE "error|exception" "$OUT/freecad_dxf.log" | grep -v FREECAD_RESULT; then echo "FAIL: FreeCAD log has errors"; fail=1; fi
else
  echo "SKIP: FreeCAD (flatpak) not available"
fi
[ "$fail" = 0 ] && echo "ALL OK" || { echo "FAILED"; exit 1; }
