#!/bin/bash
# Documentation screenshots: each scripted scene (rcad --scene, see
# crates/rcad/src/scenes.rs) rendered maximized in a private headless
# mutter (tools/headless_session.sh; the user's desktop is not touched) at
# 1280x800 and 1920x1080.
#
# The app saves the PNG once idle and then asks to close; in headless mutter
# that close is often not processed until another frame callback arrives
# (egui #8326 frame pacing), so this script waits for the PNG and then ends
# the private session itself.
#
# usage: tools/screenshots.sh [binary] [out-dir] [scenes...]
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${1:-$ROOT/target/debug/rcad}
OUT=${2:-$ROOT/artifacts/screenshots}
shift $(( $# < 2 ? $# : 2 ))  # with one argument, `shift 2` would fail and keep the binary as a scene
SCENES=${*:-params measure param_error print_export hole extrude multibody}
mkdir -p "$OUT" "$ROOT/artifacts/hs"
# One capture attempt: 0 = PNG written, 1 = no PNG within the timeout, 2 = unknown scene.
capture() { # scene size state png
  local scene=$1 size=$2 state=$3 png=$4
  rm -rf "$state" "$png"
  "$ROOT/tools/headless_session.sh" "$state" "$size" \
    "$BIN" --maximized --scene "$scene" --screenshot "$png" >/dev/null 2>&1 &
  local t0
  t0=$(date +%s)
  until [ -s "$png" ] || [ $(( $(date +%s) - t0 )) -gt "${RCAD_SHOT_TIMEOUT:-150}" ]; do sleep 1; done
  sleep 1
  local sock unit
  sock=$(cat "$state/wayland" 2>/dev/null)
  [ -n "$sock" ] && pkill -f "$sock"
  unit=$(cat "$state/unit" 2>/dev/null)
  [ -n "$unit" ] && systemctl --user stop "$unit" 2>/dev/null
  wait 2>/dev/null
  if grep -q "unknown scene" "$state/app.log" 2>/dev/null; then
    rm -f "$png"
    return 2
  fi
  [ -s "$png" ]
}

# Stalls (2026-10-12): captures that rendered no frame were the Vulkan present path under
# headless mutter (see tools/headless_session.sh, which now runs the app on wgpu's GL
# backend). Each capture still gets RCAD_SHOT_ATTEMPTS (default 2) attempts, each in a
# fresh private session, as a safety net.
for size in ${RCAD_SIZES:-1280x800 1920x1080}; do
  for scene in $SCENES; do
    state="$ROOT/artifacts/hs/$scene-$size"
    png="$OUT/$scene-$size.png"
    t0=$(date +%s)
    attempt=1
    while :; do
      capture "$scene" "$size" "$state" "$png"
      rc=$?
      [ $rc -ne 1 ] || [ "$attempt" -ge "${RCAD_SHOT_ATTEMPTS:-2}" ] && break
      echo "retry $scene $size (attempt $attempt rendered no frame)"
      attempt=$((attempt + 1))
    done
    case $rc in
      0) echo "ok   $scene $size ($(( $(date +%s) - t0 )) s, attempt $attempt)" ;;
      2) echo "FAIL $scene $size (unknown scene; see $state/app.log)" ;;
      *) echo "FAIL $scene $size after $attempt attempts (see $state/app.log)" ;;
    esac
  done
done
