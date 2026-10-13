#!/usr/bin/env bash
# Run-6 acceptance 5 (threaded pair): rod and nut (stated 60° V test profile, Ø20 x 2,
# length 12, 0.1 mm radial clearance; ADR 0011, faceted, labelled) through the CLI, then:
#   1. FreeCAD: rod and nut valid single solids at our volumes; rod ∩ nut empty in the
#      designed mating placement, control clash when the nut is moved half a pitch
#      (tools/check_thread.sh on the kernel-test exports);
#   2. the independent mesh checker on the placed 3MF;
#   3. Bambu Studio slice of the placed 3MF (P1S; sliced, not printed; no fit claim).
# Usage (repository root): tools/validate_threaded_pair.sh <rcad-cli> <outdir>
set -uo pipefail
CLI=$(readlink -f "${1:?usage: validate_threaded_pair.sh <rcad-cli> <outdir>}")
OUT=${2:?outdir}; mkdir -p "$OUT"; OUT=$(readlink -f "$OUT"); FAIL=0
mark() { if [ "$1" -ne 0 ]; then FAIL=1; echo "   -> FAIL (exit $1)"; else echo "   -> ok"; fi; }
echo "== 1. kernel exports + FreeCAD (rod, nut, mating, control)"
RCAD_EVIDENCE_DIR=$OUT tools/heavy.sh cargo test -p rcad-kernel --release --lib thread > "$OUT/kernel_test.log" 2>&1; mark $?
tools/heavy.sh tools/check_thread.sh "$OUT" | tee "$OUT/check_thread.log"; mark ${PIPESTATUS[0]}
echo "== 2. CLI workflow: rod + nut, placed 3MF and STEP"
cat > "$OUT/workflow.json" <<JSON
[
 {"cmd": "new_document"},
 {"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0, "length": 12.0},
 {"cmd": "thread", "plane": "WorldXY", "diameter": 20.0, "pitch": 2.0, "length": 12.0, "nut_width": 30.0},
 {"cmd": "export_3mf", "path": "$OUT/threaded_pair.3mf", "placement": {"layout": {"gap": 10}}, "bed": {"preset": "bambu_p1s"}},
 {"cmd": "export_step", "path": "$OUT/threaded_pair.step"}
]
JSON
"$CLI" run "$OUT/workflow.json" > "$OUT/workflow.jsonl" 2>&1
! grep -q '"as_expected":false' "$OUT/workflow.jsonl"; mark $?
python3 -I tools/print/check_print_mesh.py "$OUT/threaded_pair.3mf" > "$OUT/mesh_check.txt" 2>&1
rc=$?; tail -1 "$OUT/mesh_check.txt"; mark $rc
echo "== 3. Bambu Studio slice (sliced, not printed)"
tools/print/slice_bambu.sh "$OUT/slice" "$OUT/threaded_pair.3mf" > "$OUT/slice_run.log" 2>&1; mark $?
python3 -I tools/print/analyze_gcode.py "$OUT/slice/plate_1.gcode" --log "$OUT/slice/slice.log" \
  --layers --out "$OUT/slice/analysis.json" > "$OUT/slice/analysis.txt" 2>&1
rc=$?; tail -1 "$OUT/slice/analysis.txt"; mark $rc
[ $FAIL -eq 0 ] && echo "ALL OK" || echo "SOME STEPS FAILED"
exit $FAIL
