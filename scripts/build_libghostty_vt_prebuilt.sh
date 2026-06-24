#!/usr/bin/env bash
set -euo pipefail

# ponytail: local no-CI prebuilt producer; remove with the rest of the prebuilt
# fallback when upstream zig-0.16 port lands (ghostty PR #12726) and we
# vendor-update — at that point build.rs compiles from source again on all hosts.
#
# Cross-builds libghostty-vt.a for the macOS host target from inside a Linux
# container with zig 0.15.2. zig 0.15.2 cannot link its own build runner on
# macOS 26 (Xcode 26 SDK linker break), but it cross-builds the macOS .a fine
# from Linux. Output lands in prebuilt/libghostty-vt-<target>.a, which build.rs
# auto-detects. Fully local, no GitHub Actions required.

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

# Host arch -> macOS zig target + container platform. We only cross-build macOS
# targets here; Linux hosts build their native .a from source via build.rs.
ARCH=$(uname -m)
case "$ARCH" in
  arm64 | aarch64) ZIGTARGET=aarch64-macos; PLATFORM=linux/arm64 ;;
  x86_64)          ZIGTARGET=x86_64-macos;  PLATFORM=linux/amd64 ;;
  *)
    echo "error: unsupported host arch: $ARCH (expected arm64/aarch64 or x86_64)" >&2
    exit 1
    ;;
esac

command -v docker >/dev/null 2>&1 || {
  echo "error: docker not found; this script cross-builds inside a Linux container" >&2
  exit 1
}

OUT_DIR="$ROOT_DIR/prebuilt"
mkdir -p "$OUT_DIR"
TMP_OUT=$(mktemp -d)
trap 'rm -rf "$TMP_OUT"' EXIT

printf 'build-libghostty-vt-prebuilt: target=%s platform=%s\n' "$ZIGTARGET" "$PLATFORM"

docker run --rm -i --platform "$PLATFORM" \
  -v "$ROOT_DIR":/work:ro -v "$TMP_OUT":/out \
  -e "ZIGTARGET=$ZIGTARGET" \
  alpine:3.20 sh -s <<'INNER'
set -eu
ZIG_VER=0.15.2
ARCH=$(uname -m)   # aarch64 or x86_64 of the linux/<arch> container
apk add --no-cache curl tar xz >/dev/null
URL="https://ziglang.org/download/${ZIG_VER}/zig-${ARCH}-linux-${ZIG_VER}.tar.xz"
echo "zig url: $URL"
cd /tmp
curl -fsSL "$URL" -o zig.tar.xz
mkdir -p zig && tar xf zig.tar.xz -C zig --strip-components=1
ZIG=/tmp/zig/zig
"$ZIG" version
cd /work/vendor/libghostty-vt
"$ZIG" build -Demit-lib-vt -Doptimize=ReleaseFast -Dsimd=true \
  "-Dtarget=${ZIGTARGET}" "-Dversion-string=$(cat VERSION)" \
  -Demit-xcframework=false \
  -p /out --cache-dir /tmp/zcache --global-cache-dir /tmp/zgcache
echo "=== produced ==="
ls -l /out/lib/
INNER

cp "$TMP_OUT/lib/libghostty-vt.a" "$OUT_DIR/libghostty-vt-${ZIGTARGET}.a"
printf 'build-libghostty-vt-prebuilt: saved %s\n' "$OUT_DIR/libghostty-vt-${ZIGTARGET}.a"
