#!/bin/bash
# Run every tools/live/*.txt script against a fresh rcad in its own private
# headless compositor (tools/headless_session.sh), with real pointer/keyboard
# input injected through mutter's RemoteDesktop API (tools/live_input.py).
# Each script asserts document state via the app's RCAD_PROBE output.
# Scripts may use @OUT@ (the per-script output dir) and @ROOT@ (the repository root).
# Exit status is non-zero if any script or expectation fails.
#
# usage: tools/live_walkthrough.sh [out-dir] [script-name-filter]
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-$ROOT/artifacts/live_walkthrough}
FILTER=${2:-}
APP=$ROOT/target/release/rcad
[ -x "$APP" ] || { echo "build first: cargo build --release -p rcad"; exit 2; }
rc=0
for script in "$ROOT"/tools/live/*.txt; do
  name=$(basename "$script" .txt)
  case "$name" in *"$FILTER"*) ;; *) continue ;; esac
  dir=$OUT/$name
  # Fresh per-script directory; refuse anything that is not a child of OUT.
  case "$dir" in "$OUT"/?*) ;; *) echo "refusing to clear $dir"; exit 2 ;; esac
  if [ -d "${dir:?}" ]; then rm -rf -- "${dir:?}"; fi
  mkdir -p "$dir/shots"
  RCAD_TRACE_INPUT=1 RCAD_PROBE="$dir/probe.json" RCAD_RECOVERY_DIR="$dir/recovery" \
    "$ROOT/tools/headless_session.sh" "$dir" 1280x800 "$APP" --maximized --shot-dir "$dir/shots" \
    > "$dir/session.log" 2>&1 &
  session=$!
  for _ in $(seq 1 150); do [ -s "$dir/probe.json" ] && break; sleep 0.1; done
  sed -e "s#@OUT@#$dir#g" -e "s#@ROOT@#$ROOT#g" "$script" > "$dir/script.txt"
  SECONDS=0
  if /usr/bin/python3 "$ROOT/tools/live_input.py" "$dir" "$dir/script.txt" > "$dir/result.log" 2>&1; then
    status=PASS
  else
    status=FAIL; rc=1
  fi
  echo "$status $name (${SECONDS} s)"
  grep -E "^(PASS|FAIL) expect|Error|error" "$dir/result.log" | sed 's/^/    /'
  # Stop this script's app; the session wrapper then stops its compositor.
  pkill -f -- "--shot-dir $dir/shots"
  for _ in $(seq 1 50); do kill -0 "$session" 2>/dev/null || break; sleep 0.1; done
  kill "$session" 2>/dev/null
  # Run 4: killing the wrapper can orphan its mutter (it then lived on and,
  # under tools/heavy.sh, kept the build lock); stop it by its private socket.
  sock=$(cat "$dir/wayland" 2>/dev/null)
  [ -n "$sock" ] && pkill -f -- "--wayland-display $sock"
  # Stop the session's scope: kills whatever is left in its cgroup.
  unit=$(cat "$dir/unit" 2>/dev/null)
  [ -n "$unit" ] && systemctl --user stop "$unit" 2>/dev/null
done
exit $rc
