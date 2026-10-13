#!/usr/bin/env bash
# Verify a pinned commit without freezing the working checkout: a detached worktree
# at <commit> under .claude/worktrees/verify-<sha>, its own disk-backed target dir
# (<repo>/target-verify, reused between runs, never shared with a working tree), and the
# given verification script run inside it. Feature work continues in the main checkout
# meanwhile; heavy steps still go through tools/heavy.sh slots.
# Usage (repository root): tools/verify_at.sh <commit> <out-dir> <script> [args...]
#   e.g. tools/verify_at.sh HEAD artifacts/final tools/run5_final_verify.sh
# The script receives <out-dir> (absolute) as its first argument.
set -euo pipefail
ROOT="$(cd "$(dirname "$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir)")" && pwd)"
REV=${1:?commit}; OUT=${2:?out dir}; SCRIPT=${3:?script}; shift 3
SHA=$(git -C "$ROOT" rev-parse --short "$REV")
WT=$ROOT/.claude/worktrees/verify-$SHA
mkdir -p "$OUT"; OUT=$(readlink -f "$OUT")
if [ ! -d "$WT" ]; then
  git -C "$ROOT" worktree add --detach "$WT" "$SHA" >/dev/null
fi
[ "$(git -C "$WT" rev-parse --short HEAD)" = "$SHA" ] || { echo "verify_at: $WT is not at $SHA" >&2; exit 2; }
export CARGO_TARGET_DIR=${VERIFY_TARGET_DIR:-$ROOT/target-verify}
case "$CARGO_TARGET_DIR" in /tmp/*) echo "verify_at: refusing tmpfs target dir" >&2; exit 2;; esac
echo "verify_at: commit $SHA in $WT, target $CARGO_TARGET_DIR, out $OUT"
cd "$WT"
exec "$WT/$SCRIPT" "$OUT" "$@"
