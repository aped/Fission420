#!/usr/bin/env bash
# Run a heavy job (cargo build/test/clippy, FreeCAD or slicer validation) under a
# machine-wide limit shared by every checkout and worktree of this repository. Up to
# HEAVY_SLOTS (default 2) jobs run at once; a job takes a second or later slot only
# while MemAvailable is at least HEAVY_MIN_FREE_GB (default 8), otherwise it waits for
# slot 0 (one heavy job at a time).
# Usage: tools/heavy.sh cargo test -p rcad-document --release
# Builds use 4 jobs and the shared disk-backed target dir unless CARGO_TARGET_DIR is set
# (each worktree should set its own; cargo locks a target dir itself, so two jobs on one
# target dir simply take turns).
set -euo pipefail
# Main checkout (the parent of the shared .git), so every worktree uses the same locks and
# the same default target dir.
ROOT="$(cd "$(dirname "$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir)")" && pwd)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
case "$CARGO_TARGET_DIR" in /tmp/*) echo "heavy.sh: refusing tmpfs target dir $CARGO_TARGET_DIR" >&2; exit 2;; esac
SLOTS="${HEAVY_SLOTS:-2}"
MIN_FREE_GB="${HEAVY_MIN_FREE_GB:-8}"
mkdir -p "$ROOT/target"
lock() { # slot -> lock file (slot 0 keeps the historical name)
  if [ "$1" -eq 0 ]; then echo "$ROOT/target/.heavy-lock"; else echo "$ROOT/target/.heavy-lock-$1"; fi
}
free_gb() { awk '/^MemAvailable:/ { printf "%d", $2 / 1048576 }' /proc/meminfo; }
# Exit code flock returns when a slot is busy (-n); distinct from the command's codes in
# practice (75 = EX_TEMPFAIL).
BUSY=75
while :; do
  for ((s = 0; s < SLOTS; s++)); do
    if [ "$s" -gt 0 ] && [ "$(free_gb)" -lt "$MIN_FREE_GB" ]; then
      break
    fi
    # -o: the lock fd is not inherited by the command (a long-lived child such as a
    # headless compositor must not keep holding the lock after the job ends).
    set +e
    flock -n -E "$BUSY" -o "$(lock "$s")" nice "$@"
    rc=$?
    set -e
    if [ "$rc" -ne "$BUSY" ]; then
      exit "$rc"
    fi
  done
  sleep 2
done
