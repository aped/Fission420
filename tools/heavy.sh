#!/usr/bin/env bash
# Run one heavy job (cargo build/test/clippy, FreeCAD or slicer validation) at a time
# across the coordinator and workers (runs/run-4/SESSION_4.md resource budget).
# Usage: tools/heavy.sh cargo test -p f420-document --release
# Waits for the lock; builds use 4 jobs and the shared disk-backed target dir.
set -euo pipefail
ROOT=<repo>
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
case "$CARGO_TARGET_DIR" in /tmp/*) echo "heavy.sh: refusing tmpfs target dir $CARGO_TARGET_DIR" >&2; exit 2;; esac
exec flock "$ROOT/target/.heavy-lock" nice "$@"
