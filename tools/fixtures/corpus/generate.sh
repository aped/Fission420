#!/usr/bin/env bash
# Regenerate the qualification corpus (tools/fixtures/corpus/): FreeCAD-made
# parts/assemblies/meshes (gen_freecad.py, FreeCAD flatpak 1.1.4 / OCC 7.8.1; its
# read-back measurements go to freecad_reference.json) and hand-authored edge cases
# (gen_handwritten.py, plain python3). Second set: gen_freecad_more.py (references in
# freecad_reference_more.json) and gen_handwritten_more.py.
# usage: tools/heavy.sh tools/fixtures/corpus/generate.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
flatpak run --filesystem="$REPO" --env=RCAD_OUT="$HERE" \
    --command=FreeCADCmd org.freecad.FreeCAD "$HERE/gen_freecad.py" 2>&1 \
  | grep -a -o "RCAD_CORPUS.*" | sed 's/^RCAD_CORPUS //' \
  | python3 -I -c 'import json,sys; json.dump(json.loads(sys.stdin.read()), open(sys.argv[1],"w"), indent=1, sort_keys=True)' "$HERE/freecad_reference.json"
flatpak run --filesystem="$REPO" --env=RCAD_OUT="$HERE" \
    --command=FreeCADCmd org.freecad.FreeCAD "$HERE/gen_freecad_more.py" 2>&1 \
  | grep -a -o "RCAD_CORPUS.*" | sed 's/^RCAD_CORPUS //' \
  | python3 -I -c 'import json,sys; json.dump(json.loads(sys.stdin.read()), open(sys.argv[1],"w"), indent=1, sort_keys=True)' "$HERE/freecad_reference_more.json"
python3 -I "$HERE/gen_handwritten.py" "$HERE"
# Second set (reads c0 from gen_freecad_more.py and h1 from gen_handwritten.py).
python3 -I "$HERE/gen_handwritten_more.py" "$HERE"
echo "corpus written to $HERE"
