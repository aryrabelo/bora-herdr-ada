#!/bin/sh
set -eu

BIN="bora"
MANIFEST_URL="https://raw.githubusercontent.com/aryrabelo/bora-herdr-ada/main/website/latest.json"
INSTALL_DIR="${HERDR_INSTALL_DIR:-$HOME/.local/bin}"

main() {
    echo ""
    echo "     wWWWWWWW_)  bora installer"
    echo "     \`WWWWWW'    aryrabelo/bora-herdr-ada"
    echo "      II  II"
    echo ""

    # detect platform
    OS="$(uname -s)"
    case "$OS" in
        Linux)  os="linux" ;;
        Darwin) os="macos" ;;
        *)      err "unsupported OS: $OS" ;;
    esac

    ARCH="$(uname -m)"
    case "$ARCH" in
        x86_64|amd64)   arch="x86_64" ;;
        aarch64|arm64)  arch="aarch64" ;;
        *)              err "unsupported architecture: $ARCH" ;;
    esac

    log "detected ${os}/${arch}"

    # check dependencies
    need curl
    need awk

    # use the same manifest as `herdr update` so installs and updates agree
    # on the public latest release.
    TARGET="${os}-${arch}"
    log "fetching latest release manifest..."
    MANIFEST="$(curl -fsSL --retry 3 --connect-timeout 10 --max-time 20 "$MANIFEST_URL")" \
        || err "can't reach ${MANIFEST_URL}. Please try again later."
    URL="$(printf '%s\n' "$MANIFEST" | awk -v target="\"${TARGET}\"" '
        /^[[:space:]]*"assets"[[:space:]]*:/ { in_assets = 1; next }
        in_assets && /^[[:space:]]*}/ { exit }
        in_assets && index($0, target) {
            sub(/^.*:[[:space:]]*"/, "")
            sub(/".*$/, "")
            print
            exit
        }
    ')"
    VERSION="$(printf '%s\n' "$MANIFEST" | awk -F '"' '/^[[:space:]]*"version"[[:space:]]*:/ { print $4; exit }')"

    if [ -z "$URL" ]; then
        err "release manifest does not include a binary for ${TARGET}"
    fi

    if [ -n "$VERSION" ]; then
        log "downloading v${VERSION}..."
    else
        log "downloading latest release..."
    fi
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT

    if ! curl -fsSL --retry 3 --connect-timeout 10 --max-time 120 "$URL" -o "${TMP}/${BIN}"; then
        err "download failed from ${URL}"
    fi

    # install: stage inside $INSTALL_DIR (same fs), fix perms/signature on the
    # temp file, then atomic mv — never mutate the installed path in place
    # (stale kernel-cached signature -> SIGKILL on macOS).
    mkdir -p "$INSTALL_DIR"
    STAGED="${INSTALL_DIR}/.${BIN}-install.$$"
    mv "${TMP}/${BIN}" "$STAGED"
    chmod +x "$STAGED"
    if [ "$os" = "macos" ]; then
        # AMFI rejects linker-signed adhoc signatures (flags 0x20002) and
        # quarantined downloads; clear xattrs and re-sign ad-hoc (flags 0x2).
        xattr -cr "$STAGED" 2>/dev/null || true
        command -v codesign >/dev/null 2>&1 && codesign --force --sign - "$STAGED" 2>/dev/null || true
    fi
    mv -f "$STAGED" "${INSTALL_DIR}/${BIN}"

    # smoke-run: an invalid code signature is not a run error on macOS — the
    # kernel SIGKILLs the binary at exec (exit 137), so a silent dead install
    # would otherwise pass. Fail loudly.
    if ! "${INSTALL_DIR}/${BIN}" --version >/dev/null 2>&1; then
        rc=$?
        if [ "$rc" -eq 137 ]; then
            warn "macOS killed the binary at exec (SIGKILL, exit 137) — invalid code signature."
            warn "AppleSystemPolicy rejected it; kernel log shows 'load code signature error 2'."
            warn "inspect: log show --last 2m --predicate 'eventMessage CONTAINS \"bora\"'"
            err "re-sign a fresh copy with: codesign --force --sign -"
        else
            err "installed binary failed to run (exit $rc)"
        fi
    fi

    log "installed ${BIN} to ${INSTALL_DIR}/${BIN}"

    # check PATH
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo ""
            warn "${INSTALL_DIR} is not in your PATH"
            echo "  add it to your shell config:"
            echo ""
            echo "    export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo ""
            ;;
    esac

    # verify
    if command -v "$BIN" >/dev/null 2>&1; then
        echo ""
        log "ready. run 'bora' to get started."
    fi

    echo ""
}

log()  { printf '  \033[32m>\033[0m %s\n' "$1"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$1"; }
err()  { printf '  \033[31m✗\033[0m %s\n' "$1" >&2; exit 1; }

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "requires '$1' — install it first, or download a binary manually from https://github.com/aryrabelo/bora-herdr-ada/releases"
    fi
}

main "$@"
