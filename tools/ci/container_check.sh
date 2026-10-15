#!/usr/bin/env bash
# Reproduce a Linux CI leg locally in a clean distro container (podman), e.g.
#   tools/ci/container_check.sh debian:trixie      # the debian-13 leg
#   tools/ci/container_check.sh ubuntu:24.04       # stricter than the Ubuntu runner image
# Mounts the checkout read-write (the xtask replays rewrite examples/, as in CI) and keeps
# the toolchain, registry and target dir in a per-image cache directory on disk.
# CI_CACHE must not be on tmpfs; CARGO_BUILD_JOBS defaults to 4. See docs/CI.md.
set -euo pipefail
IMAGE="${1:?usage: $0 <image> (e.g. debian:trixie)}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TOOLCHAIN="$(sed -n 's/^  RUST_TOOLCHAIN: "\(.*\)"/\1/p' "$ROOT/.github/workflows/check.yml")"
CACHE="${CI_CACHE:-$ROOT/target-ci-${IMAGE//[:\/]/-}}"
case "$CACHE" in /tmp/*) echo "refusing tmpfs cache dir $CACHE" >&2; exit 2;; esac
mkdir -p "$CACHE"
exec podman run --rm --memory "${CI_MEMORY:-12g}" \
  -v "$ROOT:/src:Z" -v "$CACHE:/cache:Z" -w /src \
  -e TOOLCHAIN="$TOOLCHAIN" -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
  "docker.io/library/$IMAGE" bash -euo pipefail -c '
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  ca-certificates curl git build-essential fonts-liberation >/dev/null
export CARGO_HOME=/cache/cargo RUSTUP_HOME=/cache/rustup CARGO_TARGET_DIR=/cache/target
export PATH=/cache/cargo/bin:$PATH CARGO_TERM_COLOR=never CARGO_INCREMENTAL=0 RUST_BACKTRACE=1
[ -x /cache/cargo/bin/rustup ] || curl -sSf https://sh.rustup.rs |
  sh -s -- -y -q --profile minimal --default-toolchain none --no-modify-path
rustup -q toolchain install "$TOOLCHAIN" --profile minimal -c rustfmt,clippy
rustup default "$TOOLCHAIN" >/dev/null
grep PRETTY_NAME /etc/os-release; rustc -V
cargo fetch --locked
cargo build --release --locked -p rcad -p rcad-cli
cargo xtask check
'
