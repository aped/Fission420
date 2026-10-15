#!/usr/bin/env bash
# Everyday printable parts (docs/PRINT_EXPORT.md "Everyday parts"): enclosure, tray, knob,
# threaded pair, split bar (examples/<name>_workflow.json), checked end to end:
#   0. replay: rcad-cli runs every committed workflow from a scratch directory; every step
#      as expected and the saved document byte-identical to examples/<name>.rcad.json;
#   1. exports: crates/rcad-commands/tests/it/printable_corpus.rs (ignored test) writes placed
#      per-body STLs, a 3MF per part, one STEP per body, the closed-form expectations
#      (spec.json, step_expected.json) and toolpath probes per plate;
#   2. tools/print/check_print_mesh.py (independent, stdlib Python) against spec.json;
#   3. FreeCAD 1.1.4: every STEP (one valid solid, volume within 1e-6 relative, bounds) and
#      every STL (solid, no non-manifolds, no self-intersections);
#   4. Bambu Studio 2.8.2.61 headless slice of every 3MF (P1S 0.4 / 0.20mm Standard /
#      Generic PLA) with the toolpath probes; slicer log findings listed.
# Sliced is not printed: nothing here says anything about fit, strength or print quality.
# Usage (repository root): tools/print/validate_everyday_parts.sh <rcad-cli> <outdir>
# cargo, FreeCAD and the slicer run through tools/heavy.sh; cargo uses CARGO_TARGET_DIR
# (default <main checkout>/target-print). Every step runs; exit 1 if any failed.
set -uo pipefail
CLI=$(readlink -f "${1:?usage: validate_everyday_parts.sh <rcad-cli> <outdir>}")
OUT=${2:?outdir}
ROOT=$(pwd)
mkdir -p "$OUT"
OUT=$(readlink -f "$OUT")
EXP=$OUT/out
MAIN="$(cd "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")" && pwd)"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$MAIN/target-print}
PARTS="everyday_enclosure everyday_tray everyday_knob threaded_pair split_bar"
FAIL=0
SUM=$OUT/SUMMARY.txt
: > "$SUM"
note() { echo "$*" | tee -a "$SUM"; }
res() { if [ "$2" -ne 0 ]; then FAIL=1; note "  $1: FAIL (exit $2)"; else note "  $1: ok"; fi; }

note "# validate_everyday_parts.sh $(date -Is)  commit $(git rev-parse --short HEAD 2>/dev/null)"
note "## 0. replay the committed workflows with $(basename "$CLI")"
REP=$OUT/replay
rm -rf "$REP"; mkdir -p "$REP/examples"
for p in $PARTS; do
  (cd "$REP" && "$CLI" run "$ROOT/examples/${p}_workflow.json" > "$OUT/replay_$p.jsonl" 2>&1)
  rc=$?
  if [ $rc -eq 0 ] && grep -q '"as_expected":false' "$OUT/replay_$p.jsonl"; then rc=2; fi
  res "replay $p" $rc
  cmp -s "$REP/examples/$p.rcad.json" "$ROOT/examples/$p.rcad.json"
  res "saved document of $p identical to the committed one" $?
done

note "## 1. exports (cargo test --test it printable_corpus:: -- --ignored)"
rm -rf "$EXP"; mkdir -p "$EXP"
RCAD_EVERYDAY_OUT=$EXP tools/heavy.sh cargo test -p rcad-commands --release \
  --test it printable_corpus::export_everyday_parts -- --ignored > "$OUT/export.log" 2>&1
res "export test" $?
ls "$EXP" | sed 's/^/    /' >> "$SUM"

note "## 2. independent mesh checks (tools/print/check_print_mesh.py)"
python3 -I tools/print/check_print_mesh.py "$EXP"/*.stl "$EXP"/*.3mf --spec "$EXP/spec.json" \
  --json "$OUT/mesh_check.json" > "$OUT/mesh_check.log" 2>&1
rc=$?
grep -E '^SUMMARY|FAIL' "$OUT/mesh_check.log" | sed 's/^/  /' | tee -a "$SUM"
res "mesh checks" $rc

note "## 3. FreeCAD (STEP validity/volume/bounds, STL solidity/self-intersection)"
: > "$OUT/freecad.log"
for f in "$EXP"/*.step; do
  tools/heavy.sh timeout 300 flatpak run --env=RCAD_STEP="$f" --env=RCAD_EXPECT_SOLIDS=1 \
    --filesystem="$ROOT" --filesystem="$OUT" --command=FreeCADCmd org.freecad.FreeCAD \
    "$ROOT/tools/freecad_check_step.py" 2>&1 \
    | grep FREECAD_RESULT | sed "s|^|STEP $(basename "$f") |" >> "$OUT/freecad.log"
done
for f in "$EXP"/*.stl; do
  tools/heavy.sh timeout 300 flatpak run --env=RCAD_PRINT="$f" --filesystem="$ROOT" \
    --filesystem="$OUT" --command=FreeCADCmd org.freecad.FreeCAD \
    "$ROOT/tools/print/freecad_check_print.py" 2>&1 \
    | grep FREECAD_RESULT | sed "s|^|STL $(basename "$f") |" >> "$OUT/freecad.log"
done
python3 -I - "$OUT/freecad.log" "$EXP/step_expected.json" "$EXP" >> "$SUM" <<'PY'
import json, os, sys
exp = json.load(open(sys.argv[2]))
stls = {f for f in os.listdir(sys.argv[3]) if f.endswith(".stl")}
bad, seen = 0, set()
for line in open(sys.argv[1]):
    kind, name, _, payload = line.split(" ", 3)
    r = json.loads(payload)
    seen.add(name)
    if kind == "STEP":
        e = exp.get(name)
        ok = bool(r.get("ok") and r.get("isValid") and r.get("solids") == 1 and e)
        rel = abs(r.get("volume", 0) - e["volume"]) / e["volume"] if e else 1
        db = max(abs(a - b) for a, b in zip(r.get("bounds", [0] * 6), e["bounds"])) if e else 1
        ok = ok and rel <= 1e-6 and db <= e["bounds_tol"] + 1e-6
        bad += not ok
        print("    %-4s STEP %-34s solids=%s valid=%s V=%.6f expected %.6f (rel %.1e) bounds dev %.1e" % (
            "PASS" if ok else "FAIL", name, r.get("solids"), r.get("isValid"), r.get("volume", 0),
            e["volume"] if e else 0, rel, db))
    else:
        objs = r.get("objects", [])
        if "error" in r or not objs:
            bad += 1; print("    FAIL STL %s %s" % (name, r.get("error")))
        for o in objs:
            ok = o["isSolid"] and not o["hasNonManifolds"] and not o["hasSelfIntersections"] and o["volume"] > 0
            bad += not ok
            print("    %-4s STL  %-34s solid=%s nonmanifold=%s selfintersect=%s V=%.4f" % (
                "PASS" if ok else "FAIL", name, o["isSolid"], o["hasNonManifolds"],
                o["hasSelfIntersections"], o["volume"]))
for m in sorted((set(exp) | stls) - seen):
    bad += 1; print("    FAIL %s: no FreeCAD result" % m)
sys.exit(1 if bad else 0)
PY
res "FreeCAD" $?

note "## 4. Bambu Studio slicing + toolpath probes"
for p in $PARTS; do
  d=$OUT/slice_$p
  tools/print/slice_bambu.sh "$d" "$EXP/$p.3mf" > "$OUT/slice_$p.log" 2>&1
  rc=$?
  res "slice $p (exit $rc)" $rc
  [ -f "$d/result.gcode.3mf" ] || continue
  python3 -I tools/print/analyze_gcode.py "$d/result.gcode.3mf" --probes "$EXP/probes_$p.json" \
    --log "$d/slice.log" --out "$d/analysis.json" > "$OUT/analysis_$p.log" 2>&1
  rc=$?
  python3 -I - "$d/analysis.json" "$d/slice.log" >> "$SUM" <<'PY'
import json, re, sys
a = json.load(open(sys.argv[1]))
pr = a.get("probes", [])
print("    layers %s, max_z %s" % (a.get("layer_count"), a.get("header", {}).get("max_z_height")))
for m in a.get("mesh_stat", []):
    keys = ("edges_fixed", "degenerate_facets", "facets_removed", "facets_reversed", "backwards_edges")
    print("      repair counters %-40s %s" % (m.get("part", "")[:40], {k: m.get(k) for k in keys}))
print("    probes %d/%d pass" % (sum(1 for p in pr if p.get("ok")), len(pr)))
for p in pr:
    if not p.get("ok"):
        print("      FAIL probe:", p.get("name"))
for f in a.get("log", {}).get("findings", []):
    print("      slicer log (%s): %s" % ("benign" if f.get("benign") else "finding", f.get("text", "")[:200]))
# Every warning line of the slicer log, for the per-warning explanation in the docs.
seen = set()
for line in open(sys.argv[2], errors="replace"):
    if re.search(r"\[(warning|error)\]", line):
        t = re.sub(r"^\[[^\]]*\] \[[^\]]*\] ", "", line.strip())[:160]
        if t not in seen:
            seen.add(t); print("      log line:", t)
PY
  res "probes $p" $rc
done

note "## result: $([ $FAIL -eq 0 ] && echo ALL OK || echo FAIL)"
exit $FAIL
