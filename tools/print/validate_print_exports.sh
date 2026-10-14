#!/usr/bin/env bash
# End-to-end print-export validation (docs/PRINT_EXPORT.md):
#   1. rcad-cli runs tools/print/enclosure_print_workflow.json (baseline + variant enclosure/lid,
#      placed STL per body + multi-object 3MF, cylinder at chord 0.02 / 0.005);
#   2. independent Python mesh checks against analytic values (tools/print/enclosure_spec.json);
#   3. FreeCAD Mesh cross-check per file (incl. FreeCAD's self-intersection test);
#   4. Bambu Studio headless slice (P1S 0.4 / 0.20mm Standard / Generic PLA) of the baseline and
#      variant 3MF and STL sets, G-code probes for walls/cavity/floor/lip/clearance.
# Usage: tools/print/validate_print_exports.sh <rcad-cli binary> [outdir]
# (default outdir artifacts/print/validation). Run from the repository root. Every FreeCAD
# and slicer run goes through tools/heavy.sh. Exit 1 if any step fails; each step still runs.
set -uo pipefail
CLI=${1:?usage: validate_print_exports.sh <rcad-cli> [outdir]}
CLI=$(readlink -f "$CLI")
OUT=${2:-artifacts/print/validation}
ROOT=$(pwd)
mkdir -p "$OUT"
OUT=$(readlink -f "$OUT")
EXP=$ROOT/artifacts/print/exports
FAIL=0
step() { echo "== $*"; }
mark() { if [ "$1" -ne 0 ]; then FAIL=1; echo "   -> FAIL (exit $1)"; else echo "   -> ok"; fi; }

step "1. export workflow ($CLI)"
rm -rf "$EXP"; mkdir -p "$EXP"
"$CLI" run tools/print/enclosure_print_workflow.json > "$EXP/workflow.jsonl"; mark $?

step "2. independent mesh checks"
python3 -I tools/print/check_print_mesh.py "$EXP"/*.stl "$EXP"/*.3mf \
  --spec tools/print/enclosure_spec.json --json "$OUT/mesh_check.json" > "$OUT/mesh_check.txt" 2>&1
rc=$?; grep -cE '\bPASS\b' "$OUT/mesh_check.txt" | sed 's/^/   PASS lines: /'
grep -E '\bFAIL\b' "$OUT/mesh_check.txt" | head -20; mark $rc

step "3. FreeCAD cross-check"
: > "$OUT/freecad.txt"
for f in "$EXP"/*.stl "$EXP"/*.3mf; do
  tools/heavy.sh timeout 300 flatpak run --env=RCAD_PRINT="$f" --filesystem="$ROOT" \
    --command=FreeCADCmd org.freecad.FreeCAD "$ROOT/tools/print/freecad_check_print.py" 2>&1 \
    | grep FREECAD_RESULT >> "$OUT/freecad.txt"
done
python3 -I - "$OUT/freecad.txt" <<'EOF'
import json, sys
bad = 0
for line in open(sys.argv[1]):
    r = json.loads(line.split(" ", 1)[1])
    # FreeCAD 1.1.4 merges all 3MF build items into ONE mesh, so for a multi-object 3MF its
    # self-intersection flag also fires where separate objects overlap (e.g. as-modeled base
    # and lid share space). Only per-file STLs (one body each) get a self-intersection verdict.
    merged = r["file"].endswith(".3mf")
    for o in r.get("objects", []):
        si = o["hasSelfIntersections"]
        ok = o["isSolid"] and not o["hasNonManifolds"] and o["volume"] > 0 and (merged or not si)
        bad += not ok
        print("   %-28s %s solid=%s nonmanifold=%s selfintersect=%s%s V=%.4f" % (
            r["file"].rsplit("/", 1)[1], o["name"], o["isSolid"], o["hasNonManifolds"], si,
            " (merged 3MF objects: informational)" if merged else "", o["volume"]))
    if "error" in r:
        bad += 1; print("   error", r)
sys.exit(1 if bad else 0)
EOF
mark $?

step "4. Bambu Studio slicing"
for set in baseline variant; do
  for kind in 3mf stl; do
    d=$OUT/slice_${set}_$kind
    if [ $kind = 3mf ]; then models=("$EXP/$set.3mf"); else models=("$EXP/$set-Base.stl" "$EXP/$set-Lid.stl"); fi
    tools/print/slice_bambu.sh "$d" "${models[@]}" > "$d.stdout" 2>&1; rc=$?
    echo "   slice $set $kind: exit $rc"; [ $rc -ne 0 ] && FAIL=1
    if [ -f "$d/result.gcode.3mf" ]; then
      python3 -I tools/print/analyze_gcode.py "$d/result.gcode.3mf" --probes "tools/print/probes/${set}_plate.json" \
        --log "$d/slice.log" --out "$d/analysis.json" > "$d/analysis.txt" 2>&1; rc=$?
      sed 's/^/   /' "$d/analysis.txt" | cut -c1-200
      mark $rc
    fi
  done
done
echo "validate_print_exports.sh: $([ $FAIL -eq 0 ] && echo ALL OK || echo FAILURES) (logs in $OUT)"
exit $FAIL
