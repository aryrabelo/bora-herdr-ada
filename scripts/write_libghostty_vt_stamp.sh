#!/usr/bin/env bash
set -euo pipefail

# ponytail: stamp writer for the prebuilt libghostty-vt guard in
# crates/ghostty-vt/build.rs;
# remove with the rest of the prebuilt fallback when upstream zig-0.16 port
# lands (ghostty PR #12726) and we vendor-update.
#
# Writes prebuilt/libghostty-vt-<target>.vendor-hash next to a freshly
# produced/fetched .a, so the crate build.rs's stamp guard accepts it. That
# build.rs has no external crate deps, so it is compiled and run standalone here in
# `--write-stamp` mode — this reuses build.rs's own hash function as the
# single source of truth, so this script and the guard it feeds can never
# compute a different hash for the same vendor tree.

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
TARGET="${1:?usage: write_libghostty_vt_stamp.sh <zig-target>}"

TOOL=$(mktemp -t libghostty-vt-stamp-tool.XXXXXX)
trap 'rm -f "$TOOL"' EXIT

BUILD_RS="$ROOT_DIR/crates/ghostty-vt/build.rs"
rustc --edition 2021 "$BUILD_RS" -o "$TOOL"
CARGO_MANIFEST_DIR="$ROOT_DIR/crates/ghostty-vt" "$TOOL" --write-stamp "$TARGET"
