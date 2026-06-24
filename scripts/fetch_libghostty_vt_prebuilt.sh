#!/usr/bin/env bash
set -euo pipefail

# ponytail: prebuilt fetch helper; remove when upstream zig-0.16 port lands (ghostty PR #12726)
# and we vendor-update — at that point, build.rs will compile from source again on all hosts.

ROOT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

# 1. Read source_commit from vendor JSON (8-char prefix used in asset names)
VENDOR_JSON="$ROOT_DIR/vendor/libghostty-vt.vendor.json"
if [[ ! -f "$VENDOR_JSON" ]]; then
  echo "error: $VENDOR_JSON not found" >&2
  exit 1
fi
SOURCE_COMMIT=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['source_commit'])" "$VENDOR_JSON")
COMMIT8="${SOURCE_COMMIT:0:8}"

# 2. Detect host ZIGTARGET
OS=$(uname -s)
ARCH=$(uname -m)
case "$OS-$ARCH" in
  Darwin-arm64)   ZIGTARGET=aarch64-macos ;;
  Darwin-x86_64)  ZIGTARGET=x86_64-macos ;;
  Linux-aarch64)  ZIGTARGET=aarch64-linux-musl ;;
  Linux-x86_64)   ZIGTARGET=x86_64-linux-musl ;;
  *)
    echo "error: unsupported host OS/arch: $OS/$ARCH" >&2
    echo "       supported: Darwin/arm64, Darwin/x86_64, Linux/aarch64, Linux/x86_64" >&2
    exit 1
    ;;
esac

# 3. Derive owner/repo from fork remote (fall back to origin)
parse_gh_owner_repo() {
  local url="$1"
  # handles: git@github.com:OWNER/REPO.git and https://github.com/OWNER/REPO[.git]
  if [[ "$url" =~ ^git@github\.com:([^/]+)/([^.]+)(\.git)?$ ]]; then
    echo "${BASH_REMATCH[1]}/${BASH_REMATCH[2]}"
  elif [[ "$url" =~ ^https://github\.com/([^/]+)/([^/.]+)(\.git)?$ ]]; then
    echo "${BASH_REMATCH[1]}/${BASH_REMATCH[2]}"
  else
    echo "error: cannot parse GitHub owner/repo from remote URL: $url" >&2
    return 1
  fi
}

REMOTE_USED=fork
if ! REMOTE_URL=$(git -C "$ROOT_DIR" remote get-url fork 2>/dev/null); then
  REMOTE_USED=origin
  REMOTE_URL=$(git -C "$ROOT_DIR" remote get-url origin 2>/dev/null) || {
    echo "error: no 'fork' or 'origin' git remote found" >&2
    exit 1
  }
fi
OWNER_REPO=$(parse_gh_owner_repo "$REMOTE_URL") || exit 1

# 4. Download into prebuilt/
ASSET="libghostty-vt-${ZIGTARGET}-${COMMIT8}.a"
URL="https://github.com/${OWNER_REPO}/releases/download/libghostty-vt-prebuilts/${ASSET}"
OUT_DIR="$ROOT_DIR/prebuilt"
OUT="$OUT_DIR/libghostty-vt-${ZIGTARGET}.a"

printf 'fetch-libghostty-vt: remote=%s  owner/repo=%s\n' "$REMOTE_USED" "$OWNER_REPO"
printf 'fetch-libghostty-vt: commit=%s  target=%s\n'     "$COMMIT8"     "$ZIGTARGET"
printf 'fetch-libghostty-vt: url=%s\n'                   "$URL"
printf 'fetch-libghostty-vt: dest=%s\n'                  "$OUT"

mkdir -p "$OUT_DIR"

if ! curl -fL --progress-bar -o "$OUT" "$URL"; then
  printf '\n' >&2
  printf 'error: download failed for %s\n'              "$ASSET"                         >&2
  printf '       URL: %s\n'                             "$URL"                           >&2
  printf '\n'                                                                             >&2
  printf '       The prebuilt for commit %s (target %s) has not been published yet.\n'   "$COMMIT8" "$ZIGTARGET" >&2
  printf '       Trigger the libghostty-vt-prebuilts GitHub Actions workflow on the\n'              >&2
  printf '       fork repo (%s) to build and publish the asset,\n'                       "$OWNER_REPO" >&2
  printf '       then re-run:  just fetch-libghostty-vt\n'                                           >&2
  exit 1
fi

printf 'fetch-libghostty-vt: saved %s\n' "$OUT"
