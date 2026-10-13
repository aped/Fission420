#!/usr/bin/env bash
# Linux x86_64 release candidate (Run-8 §6 pull-forward): build rcad + rcad-cli from a
# clean tree with the git revision embedded (RCAD_GIT_REV -> `--version`), stage binaries,
# licenses, third-party license list (from `cargo metadata`), user docs and examples, run
# smoke tests from the staged package, and write SHA-256 checksums. Nothing is uploaded,
# signed or published. Binaries never enter git (artifacts/ is ignored).
# Usage: tools/release.sh [out-dir] [--allow-dirty]   (from the repo root)
set -euo pipefail
OUT=${1:-artifacts/release}
ALLOW_DIRTY=${2:-}
ROOT=$(pwd)
if [ -n "$(git status --porcelain --untracked-files=no)" ] && [ "$ALLOW_DIRTY" != "--allow-dirty" ]; then
  echo "refusing: tracked files have uncommitted changes (use --allow-dirty for a test build)" >&2
  exit 2
fi
REV=$(git rev-parse --short HEAD)
[ -z "$(git status --porcelain --untracked-files=no)" ] || REV="$REV-dirty"
VERSION=$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' Cargo.toml)
NAME="r-cad-$VERSION-$REV-linux-x86_64"
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target-release}
case "$CARGO_TARGET_DIR" in /tmp/*) echo "refusing tmpfs target dir" >&2; exit 2;; esac
echo "building $NAME (target $CARGO_TARGET_DIR)"
RCAD_GIT_REV=$REV tools/heavy.sh cargo build --release --locked -p rcad -p rcad-cli
mkdir -p "$OUT"
OUT=$(readlink -f "$OUT")
STAGE="$OUT/$NAME"
rm -rf -- "${STAGE:?}"
mkdir -p "$STAGE/bin" "$STAGE/docs" "$STAGE/examples"
cp "$CARGO_TARGET_DIR/release/rcad" "$CARGO_TARGET_DIR/release/rcad-cli" "$STAGE/bin/"
cp LICENSE-MIT LICENSE-APACHE README.md "$STAGE/"
for d in INSTALL COMMANDS FORMAT_SUPPORT PRINT_EXPORT PARAMETERS CONFIGURATIONS ASSEMBLIES \
         DRAWINGS SHEET_METAL SURFACES MESH MATERIALS TOLERANCES UI DEPENDENCIES; do
  [ -f "docs/$d.md" ] && cp "docs/$d.md" "$STAGE/docs/"
done
cp examples/*.rcad.json examples/*_workflow.json "$STAGE/examples/"
# Third-party licenses of every resolved package (name, version, license, source).
cargo metadata --format-version 1 --locked --offline 2>/dev/null | python3 -I -c '
import json, sys
m = json.load(sys.stdin)
ws = set(m["workspace_members"])
rows = sorted((p["name"], p["version"], p.get("license") or "SEE SOURCE", p.get("repository") or "")
              for p in m["packages"] if p["id"] not in ws)
print("Third-party crates compiled into R-CAD builds (from Cargo.lock; licenses as declared).")
print("Full license texts: the crates sources (cargo vendor) or their repositories.\n")
for r in rows:
    print("%-32s %-14s %-40s %s" % r)
' > "$STAGE/THIRD_PARTY_LICENSES.txt"
# Smoke tests from the staged package (temporary copy of the examples).
SMOKE=$(mktemp -d "$ROOT/artifacts/release-smoke.XXXXXX")
trap 'rm -rf -- "$SMOKE"' EXIT
"$STAGE/bin/rcad-cli" --version | tee "$STAGE/VERSION"
grep -q "$REV" "$STAGE/VERSION"
"$STAGE/bin/rcad" --version | grep -q "$REV"
mkdir -p "$SMOKE/examples" "$SMOKE/artifacts"
cp "$STAGE"/examples/* "$SMOKE/examples/"
(cd "$SMOKE" && "$STAGE/bin/rcad-cli" run examples/plate_workflow.json > plate.log)
cmp "$SMOKE/examples/plate.rcad.json" examples/plate.rcad.json
(cd "$SMOKE" && "$STAGE/bin/rcad-cli" run examples/drawing_set_workflow.json > drawing_set.log)
[ -s "$SMOKE/artifacts/drawing_set_part.pdf" ]
echo "smoke: --version, plate example (byte-identical native file), drawing set example: OK"
(cd "$STAGE" && find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS)
tar -C "$OUT" -czf "$OUT/$NAME.tar.gz" "$NAME"
(cd "$OUT" && sha256sum "$NAME.tar.gz" > "$NAME.tar.gz.sha256")
echo "release candidate: $OUT/$NAME.tar.gz"
cat "$OUT/$NAME.tar.gz.sha256"
