#!/usr/bin/env bash
# Run one heavy job (cargo build/test/clippy, FreeCAD or slicer validation) at a time
# across the coordinator and workers (the run specification's resource budget).
# Usage: tools/heavy.sh cargo test -p f420-document --release
# Waits for the lock; builds use 4 jobs and the shared disk-backed target dir.
set -euo pipefail
# Main checkout (the parent of the shared .git), so every worktree uses the same lock and
# the same default target dir.
ROOT="$(cd "$(dirname "$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir)")" && pwd)"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
case "$CARGO_TARGET_DIR" in /tmp/*) echo "heavy.sh: refusing tmpfs target dir $CARGO_TARGET_DIR" >&2; exit 2;; esac
# -o: the lock fd is not inherited by the command (a long-lived child such as a
# headless compositor must not keep holding the lock after the job ends).
exec flock -o "$ROOT/target/.heavy-lock" nice "$@"
