#!/usr/bin/env bash
# bora autoupdate: bring this machine's bora to the newest version, unattended.
# Run daily by the `autoupdate` install component
# (scripts/install/components/60-autoupdate.sh); safe to run by hand. Never
# prompts, idempotent, bash 3.2-safe (launchd runs it with macOS /bin/bash).
# Every step is logged with a timestamp to
# ${XDG_STATE_HOME:-$HOME/.local/state}/bora/autoupdate.log.
#
# SOURCE mode (this checkout is a git repo and cargo is on PATH): only when
# the checkout is on `main`, has no uncommitted changes to tracked files and
# tracks an upstream, fetch and fast-forward it. HEAD unchanged and
# ~/.local/bin/bora present = "up to date", nothing built. Otherwise build and
# install with `scripts/install.sh --only binary`, then
# `bora server live-handoff` when a server is running. Another branch, a dirty
# tree, local commits not in upstream: logged, exit 0, nothing touched (your
# own work wins). Fetch or build failure: logged, exit 1; a failed build is
# retried by the next run even though HEAD no longer moves.
#
# RELEASE mode (no checkout or no cargo): `bora update --handoff`.
#
# Why SOURCE mode never calls `bora update`: on the preview channel the
# updater compares BUILD IDs, not versions (release_info_from_preview_manifest
# in src/update.rs), and a locally built binary has no build id, so it is
# always "outdated" and gets replaced by the latest release, i.e. DOWNGRADED.
# Measured 2026-09-15: a 0.47.0 source build on the preview channel was
# offered 0.46.8-preview.<build>. For a release install that is the right
# contract; for a source build it rolls code back.
#
# Two runs never overlap: an mkdir lock in the state directory, reclaimed when
# its holder is dead or the lock is older than a day.
set -u -o pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/bora"
LOG="$STATE_DIR/autoupdate.log"
LOCK="$STATE_DIR/autoupdate.lock"
# Present while a pulled commit is not installed yet (build failed or was
# interrupted): the next run builds even though HEAD did not move.
PENDING="$STATE_DIR/autoupdate.build-pending"
# What the `binary` install component writes.
INSTALLED_BIN="$HOME/.local/bin/bora"

log() {
    local line
    line="$(date '+%Y-%m-%d %H:%M:%S %z') $*"
    printf '%s\n' "$line" >>"$LOG"
    printf '%s\n' "$line"
}

# run CMD... — a command whose own output belongs in the log, never a prompt.
run() {
    "$@" >>"$LOG" 2>&1 </dev/null
}

acquire_lock() {
    local holder
    if mkdir "$LOCK" 2>/dev/null; then
        printf '%s\n' "$$" >"$LOCK/pid"
        return 0
    fi
    holder="$(cat "$LOCK/pid" 2>/dev/null || true)"
    # A missing pid is a holder between its mkdir and its pid write: held.
    if [ -z "$(find "$LOCK" -maxdepth 0 -mmin +1440 2>/dev/null)" ]; then
        if [ -z "$holder" ] || kill -0 "$holder" 2>/dev/null; then
            return 1
        fi
    fi
    # Stale: the holder died without its EXIT trap (kill -9, power loss), or
    # the pid was reused after a reboot (no run takes a day).
    log "reclaiming stale lock $LOCK (pid ${holder:-unknown})"
    rm -rf "$LOCK"
    mkdir "$LOCK" 2>/dev/null || return 1
    printf '%s\n' "$$" >"$LOCK/pid"
}

release_lock() {
    rm -rf "$LOCK"
}

# handoff BIN — move a running server onto the freshly installed binary.
# No server is a legitimate state, logged so silence never means success.
handoff() {
    local status
    status="$("$1" status server --json 2>>"$LOG" </dev/null || true)"
    case "$status" in
        *'"running":true'*) ;;
        *)
            log "no bora server running: nothing to hand off (ok)"
            return 0
            ;;
    esac
    if run "$1" server live-handoff; then
        log "handed the running server off to the new binary"
    else
        log "failed: bora server live-handoff; the server keeps running the old binary"
        return 1
    fi
}

source_update() {
    local branch upstream before target
    cd "$ROOT" || exit 1
    branch="$(git symbolic-ref --short -q HEAD || true)"
    if [ "$branch" != main ]; then
        log "skipped: $ROOT is on ${branch:-a detached HEAD}, not main; left alone"
        exit 0
    fi
    if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
        log "skipped: $ROOT has uncommitted changes; left alone"
        exit 0
    fi
    if ! upstream="$(git rev-parse --abbrev-ref --symbolic-full-name '@{u}' 2>/dev/null)"; then
        log "skipped: main in $ROOT tracks no upstream; left alone"
        exit 0
    fi
    if ! run git fetch --quiet; then
        log "failed: git fetch for $upstream (offline?); see the git output above"
        exit 1
    fi
    before="$(git rev-parse HEAD)"
    target="$(git rev-parse '@{u}')"
    if [ "$before" != "$target" ]; then
        if ! git merge-base --is-ancestor HEAD '@{u}'; then
            log "skipped: main has commits that are not in $upstream (not a fast-forward); left alone"
            exit 0
        fi
        # `git pull --ff-only` after the fetch above, minus pull.rebase and
        # other pull config that can turn a pull into a rebase.
        if ! run git merge --ff-only '@{u}'; then
            log "skipped: fast-forward to $upstream refused (an untracked file in the way?); left alone"
            exit 0
        fi
        log "pulled ${before:0:12}..${target:0:12} from $upstream"
        : >"$PENDING"
    elif [ -x "$INSTALLED_BIN" ] && [ ! -e "$PENDING" ]; then
        log "up to date (${before:0:12}, $("$INSTALLED_BIN" --version 2>/dev/null))"
        exit 0
    elif [ -e "$PENDING" ]; then
        log "retrying the build of ${before:0:12}: the last pulled commit was never installed"
    else
        log "no $INSTALLED_BIN yet: building ${before:0:12}"
        : >"$PENDING"
    fi
    log "building: scripts/install.sh --only binary"
    if ! run "$ROOT/scripts/install.sh" --only binary; then
        log "failed: scripts/install.sh --only binary; previous binary kept, the next run retries"
        exit 1
    fi
    rm -f "$PENDING"
    log "installed $("$INSTALLED_BIN" --version 2>/dev/null)"
    handoff "$INSTALLED_BIN" || exit 1
}

release_update() {
    local reason="$1" bin
    if bin="$(command -v bora 2>/dev/null)"; then
        :
    elif [ -x "$INSTALLED_BIN" ]; then
        bin="$INSTALLED_BIN"
    else
        log "failed: release mode ($reason) but no bora on PATH or at $INSTALLED_BIN"
        exit 1
    fi
    log "release mode ($reason): $bin update --handoff"
    if ! run "$bin" update --handoff; then
        log "failed: $bin update --handoff; see its output above"
        exit 1
    fi
    log "done: $("$bin" --version 2>/dev/null)"
}

main() {
    if ! mkdir -p "$STATE_DIR"; then
        echo "autoupdate: cannot create $STATE_DIR" >&2
        exit 1
    fi
    if ! acquire_lock; then
        log "skipped: another autoupdate run holds $LOCK (pid $(cat "$LOCK/pid" 2>/dev/null || echo unknown))"
        exit 0
    fi
    trap release_lock EXIT
    trap 'exit 130' INT TERM
    if ! command -v git >/dev/null 2>&1; then
        release_update "git not on PATH"
    elif [ "$(git -C "$ROOT" rev-parse --show-toplevel 2>/dev/null)" != "$ROOT" ]; then
        release_update "$ROOT is not a git checkout"
    elif ! command -v cargo >/dev/null 2>&1; then
        release_update "cargo not on PATH"
    else
        source_update
    fi
}

main "$@"
