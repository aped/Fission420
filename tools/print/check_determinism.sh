#!/usr/bin/env bash
# Byte-determinism of exports: the same committed document
# exported by two separate rcad-cli processes, and once more after a native
# save + reopen, must give identical STL, 3MF and STEP bytes (STEP with the same
# SOURCE_DATE_EPOCH, which only sets the header time stamp). Also prints each
# file's SHA-256 so the evidence can be compared across runs.
# Usage (repository root): tools/print/check_determinism.sh <out-dir> [doc.rcad.json ...]
# Exit 1 on any difference.
set -uo pipefail
OUT=${1:?out dir}; shift
DOCS=("$@")
[ ${#DOCS[@]} -gt 0 ] || DOCS=(examples/organizer_tray.rcad.json examples/organizer_tray_finished.rcad.json examples/enclosure.rcad.json)
CLI=${RCAD_CLI:-${CARGO_TARGET_DIR:-target}/release/rcad-cli}
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-1760000000}
mkdir -p "$OUT"
OUT=$(readlink -f "$OUT")
rc=0
exports() { # dir -> JSON export steps
  local d=$1
  cat <<J
{"cmd": "export_stl", "path": "$d/doc.stl", "placement": {"bodies": [], "layout": {"gap": 10}}, "bed": {"preset": "bambu_p1s"}},
{"cmd": "export_3mf", "path": "$d/doc.3mf", "placement": {"bodies": [], "layout": {"gap": 10}}, "bed": {"preset": "bambu_p1s"}},
{"cmd": "export_step", "path": "$d/doc.step"}
J
}
for doc in "${DOCS[@]}"; do
  name=$(basename "$doc" .rcad.json)
  base=$OUT/$name
  rm -rf "$base"; mkdir -p "$base"/{first,second,reopened}
  for r in first second; do
    { echo '['; echo "{\"cmd\": \"open\", \"path\": \"$(readlink -f "$doc")\"},"; exports "$base/$r"; echo ']'; } > "$base/$r.json"
    "$CLI" run "$base/$r.json" > "$base/$r.log" 2>&1 || { echo "FAIL $name $r: cli exit $?"; rc=1; }
  done
  { echo '['; echo "{\"cmd\": \"open\", \"path\": \"$(readlink -f "$doc")\"},";
    echo "{\"cmd\": \"save\", \"path\": \"$base/resaved.rcad.json\"},";
    echo '{"cmd": "close"},';
    echo "{\"cmd\": \"open\", \"path\": \"$base/resaved.rcad.json\"},"; exports "$base/reopened"; echo ']'; } > "$base/reopened.json"
  "$CLI" run "$base/reopened.json" > "$base/reopened.log" 2>&1 || { echo "FAIL $name reopened: cli exit $?"; rc=1; }
  for f in doc.stl doc.3mf doc.step; do
    s1=$(sha256sum < "$base/first/$f" | cut -c1-16)
    for r in second reopened; do
      if cmp -s "$base/first/$f" "$base/$r/$f"; then
        echo "PASS $name $f: first == $r (sha256 $s1…, $(stat -c %s "$base/first/$f") bytes)"
      else
        echo "FAIL $name $f: first != $r"; rc=1
      fi
    done
  done
  # The resaved document is the same document (byte-identical save).
  if cmp -s <(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["document"], sort_keys=True))' "$doc") \
            <(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["document"], sort_keys=True))' "$base/resaved.rcad.json"); then
    echo "PASS $name: resaved document identical"
  else
    echo "FAIL $name: resaved document differs"; rc=1
  fi
done
exit $rc
