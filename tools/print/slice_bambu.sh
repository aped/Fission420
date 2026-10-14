#!/usr/bin/env bash
# Headless slice with the Bambu Studio flatpak (com.bambulab.BambuStudio) for print-export validation.
#
# Usage: tools/print/slice_bambu.sh <outdir> <model.stl|model.3mf>...
#
# Profiles (system BBL presets shipped inside the flatpak, flattened via bbl_profile_flatten.py
# into <outdir>/profiles/ because the CLI wants self-contained configs; vendor files untouched):
#   machine : Bambu Lab P1S 0.4 nozzle
#   process : 0.20mm Standard @BBL X1C   (compatible_printers includes "Bambu Lab P1S 0.4 nozzle")
#   filament: Generic PLA                (compatible_printers includes "Bambu Lab P1S 0.4 nozzle")
# Override with env MACHINE_JSON / PROCESS_JSON / FILAMENT_JSON (vendor file names or paths).
#
# CLI flags used (all from `bambu-studio --help`, see artifacts/print/slicer/bambu_help.txt):
#   --datadir <outdir>/datadir  isolated settings dir: the user's real Bambu config is never read/written
#   --slice 0 --orient 0 --arrange ${ARRANGE:-0} --load-settings --load-filaments --outputdir --export-3mf
# --orient 0: never auto-orient (keep our export orientation).
# --arrange 0 (default): no re-arrange. See artifacts/print/slicer/profiles.md for what the
# CLI does with positions (STL vs 3MF).
#
# Outputs in <outdir>: slice.log (version, profile paths + inheritance chains, command line, exit
# status, full stdout/stderr), result.gcode.3mf, plate_N.gcode extracted from it, profiles/.
# All slicer work runs under tools/heavy.sh (flock serialiser shared with cargo builds).
# Never talks to a printer: no --send/upload flags exist in this flow and none are used.
set -uo pipefail

if [ $# -lt 2 ]; then
  sed -n '2,4p' "$0" >&2
  exit 2
fi

HERE="$(cd "$(dirname "$(readlink -f "$0")")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
HEAVY="$ROOT/tools/heavy.sh"
APP=com.bambulab.BambuStudio
BIN=bambu-studio
ARRANGE="${ARRANGE:-0}"

mkdir -p "$1"
OUT="$(readlink -f "$1")"
shift
MODELS=()
for m in "$@"; do
  [ -f "$m" ] || { echo "slice_bambu.sh: no such model: $m" >&2; exit 2; }
  MODELS+=("$(readlink -f "$m")")
done

LOC="$(flatpak info --show-location "$APP")" || { echo "slice_bambu.sh: $APP not installed" >&2; exit 3; }
PROF="$LOC/files/share/BambuStudio/profiles/BBL"
resolve() { case "$2" in /*) echo "$2";; *) echo "$PROF/$1/$2";; esac; }
MACHINE_SRC="$(resolve machine "${MACHINE_JSON:-Bambu Lab P1S 0.4 nozzle.json}")"
PROCESS_SRC="$(resolve process "${PROCESS_JSON:-0.20mm Standard @BBL X1C.json}")"
FILAMENT_SRC="$(resolve filament "${FILAMENT_JSON:-Generic PLA.json}")"

LOG="$OUT/slice.log"
PD="$OUT/profiles"
mkdir -p "$PD" "$OUT/datadir"
rm -f "$OUT/result.gcode.3mf" "$OUT"/plate_*.gcode

{
  echo "# slice_bambu.sh $(date -Is)"
  echo "app: $APP  $(flatpak info "$APP" | sed -n 's/^ *Version: //p')  commit $(flatpak info "$APP" | sed -n 's/^ *Commit: //p')"
  echo "location: $LOC"
  echo "models:"; printf '  %s\n' "${MODELS[@]}"
  for kind in machine process filament; do
    case $kind in machine) src=$MACHINE_SRC;; process) src=$PROCESS_SRC;; filament) src=$FILAMENT_SRC;; esac
    echo "== $kind profile: $src"
    python3 -I "$HERE/bbl_profile_flatten.py" "$src" -o "$PD/$kind.json" --provenance "$PD/$kind.provenance.json" \
      --keys name,printable_area,printable_height,bed_exclude_area,layer_height,line_width,compatible_printers \
      || { echo "flatten failed for $src"; exit 4; }
  done
} > "$LOG" 2>&1 || { cat "$LOG" >&2; exit 4; }

FS=(--filesystem="$OUT")
for m in "${MODELS[@]}"; do FS+=(--filesystem="$(dirname "$m")":ro); done

CMD=("$HEAVY" flatpak run "${FS[@]}" --command="$BIN" "$APP"
     --datadir "$OUT/datadir" --debug 3
     --slice 0 --orient 0 --arrange "$ARRANGE"
     --load-settings "$PD/machine.json;$PD/process.json"
     --load-filaments "$PD/filament.json"
     --outputdir "$OUT" --export-3mf result.gcode.3mf
     "${MODELS[@]}")

{
  printf 'command:'; printf ' %q' "${CMD[@]}"; echo
  echo "---- slicer stdout/stderr ----"
} >> "$LOG"
"${CMD[@]}" >> "$LOG" 2>&1
RC=$?
echo "---- exit status: $RC ----" >> "$LOG"

# Extract plate G-code (Metadata/plate_N.gcode) next to the 3MF.
if [ -f "$OUT/result.gcode.3mf" ]; then
  python3 -I - "$OUT/result.gcode.3mf" "$OUT" >> "$LOG" 2>&1 <<'EOF'
import re, sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
for n in z.namelist():
    m = re.fullmatch(r"Metadata/(plate_\d+\.gcode)", n)
    if m:
        open(f"{sys.argv[2]}/{m.group(1)}", "wb").write(z.read(n))
        print("extracted", n)
EOF
else
  echo "no result.gcode.3mf produced" >> "$LOG"
  [ $RC -eq 0 ] && RC=5
fi
echo "slice_bambu.sh: exit $RC, log $LOG" >&2
exit $RC
