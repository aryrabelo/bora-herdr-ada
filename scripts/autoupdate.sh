#!/usr/bin/env bash
# bora autoupdate: bring this machine's bora to the newest version, unattended.
# Run daily by the `autoupdate` install component
# (scripts/install/components/60-autoupdate.sh); safe to run by hand. Never
# prompts, idempotent, bash 3.2-safe (launchd runs it with macOS /bin/bash).
# Every step is logged with a timestamp to
# ${XDG_STATE_HOME:-$HOME/.local/state}/bora/autoupdate.log.
#
# SOURCE mode (this script lives in a git checkout): needs git and cargo on
# PATH, else it logs the error and exits 1. Only when the checkout is on
# `main`, has no uncommitted changes to tracked files and `main` tracks a
# remote's `main` (refs/remotes/<remote>/main), fetch and fast-forward it.
# HEAD unchanged and ~/.local/bin/bora present = "up to date", nothing built.
# Otherwise build and install with `scripts/install.sh --only binary`, then
# `bora server live-handoff` when a server is running. Another branch, a dirty
# tree, an upstream that is not a remote `main`, local commits not in
# upstream: logged, exit 0, nothing touched (your own work wins). Fetch,
# build, status or handoff failure: logged, exit 1; a failed build or handoff
# is retried by the next run even though HEAD no longer moves.
#
# RELEASE mode (this script is NOT in a git checkout): `bora update --handoff`.
#
# Why a checkout never reaches `bora update`, not even when cargo is missing:
# on the preview channel the updater compares BUILD IDs, not versions
# (release_info_from_preview_manifest in src/update.rs), and a locally built
# binary has no build id, so it is always "outdated" and gets replaced by the
# latest release, i.e. DOWNGRADED. Measured 2026-09-15: a 0.47.0 source build
# on the preview channel was offered 0.46.8-preview.<build>. For a release
# install that is the right contract; for a source build it rolls code back,
# and it would also skip every branch and dirty-tree guard below.
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
# Present while an installed binary has not reached a running server (status
# or handoff failed): the next run retries the handoff.
HANDOFF_PENDING="$STATE_DIR/autoupdate.handoff-pending"
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

take_lock() {
    mkdir "$LOCK" 2>/dev/null || return 1
    printf '%s\n' "$$" >"$LOCK/pid"
}

# lock_is_stale — the holder died without its EXIT trap (kill -9, power loss),
# or the lock is older than a day (no run takes a day; covers a pid reused
# after a reboot). A missing pid is a holder between its mkdir and its pid
# write: held, until the age rule.
lock_is_stale() {
    local holder
    [ -n "$(find "$LOCK" -maxdepth 0 -mmin +1440 2>/dev/null)" ] && return 0
    holder="$(cat "$LOCK/pid" 2>/dev/null || true)"
    [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null
}

acquire_lock() {
    local reclaim="$LOCK.reclaim"
    take_lock && return 0
    lock_is_stale || return 1
    # Reclaiming is itself serialized: two runs that both saw the same stale
    # lock must not both remove it, or the second removes the lock the first
    # just took. The staleness is re-checked inside the reclaim lock. A
    # reclaim lock older than an hour belongs to a run killed mid-reclaim
    # (reclaiming takes milliseconds).
    if [ -n "$(find "$reclaim" -maxdepth 0 -mmin +60 2>/dev/null)" ]; then
        rmdir "$reclaim" 2>/dev/null || true
    fi
    mkdir "$reclaim" 2>/dev/null || return 1
    if lock_is_stale; then
        log "reclaiming stale lock $LOCK (pid $(cat "$LOCK/pid" 2>/dev/null || echo unknown))"
        rm -rf "$LOCK"
    fi
    if take_lock; then
        rmdir "$reclaim"
        return 0
    fi
    rmdir "$reclaim"
    return 1
}

release_lock() {
    rm -rf "$LOCK"
}

# handoff BIN — move a running server onto the freshly installed binary.
# Only a successful `status server` that says not running means "no server";
# a status error is a failure, never mistaken for an idle machine. Every
# failure leaves HANDOFF_PENDING so the next run retries.
handoff() {
    local status
    : >"$HANDOFF_PENDING"
    if ! status="$("$1" status server --json 2>>"$LOG" </dev/null)"; then
        log "failed: $1 status server --json; handoff retried next run"
        return 1
    fi
    case "$status" in
        *'"running":true'*)
            if ! run "$1" server live-handoff; then
                log "failed: bora server live-handoff; the server keeps running the old binary, retried next run"
                return 1
            fi
            log "handed the running server off to the new binary"
            ;;
        *'"running":false'*)
            log "no bora server running: nothing to hand off (ok)"
            ;;
        *)
            log "failed: unrecognized bora status server --json output; handoff retried next run"
            return 1
            ;;
    esac
    rm -f "$HANDOFF_PENDING"
}

source_update() {
    local branch remote upstream before target had_pending=0
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
    if ! upstream="$(git rev-parse --symbolic-full-name '@{u}' 2>/dev/null)"; then
        log "skipped: main in $ROOT tracks no upstream; left alone"
        exit 0
    fi
    # Only a remote's main: a main that tracks a feature branch or a local
    # branch would otherwise be fast-forwarded onto it and built.
    remote="$(git config branch.main.remote || true)"
    if [ -z "$remote" ] || [ "$remote" = . ] || [ "$upstream" != "refs/remotes/$remote/main" ]; then
        log "skipped: main in $ROOT tracks ${upstream#refs/}, not a remote's main; left alone"
        exit 0
    fi
    upstream="$remote/main"
    if ! run git fetch --quiet "$remote"; then
        log "failed: git fetch $remote (offline?); see the git output above"
        exit 1
    fi
    before="$(git rev-parse HEAD)"
    target="$(git rev-parse '@{u}')"
    if [ "$before" != "$target" ]; then
        if ! git merge-base --is-ancestor HEAD '@{u}'; then
            log "skipped: main has commits that are not in $upstream (not a fast-forward); left alone"
            exit 0
        fi
        # Marked before the merge: a run killed between the merge and the
        # install still builds next time, though HEAD will not move again.
        [ -e "$PENDING" ] && had_pending=1
        : >"$PENDING"
        # `git pull --ff-only` after the fetch above, minus pull.rebase and
        # other pull config that can turn a pull into a rebase.
        if ! run git merge --ff-only '@{u}'; then
            [ "$had_pending" = 1 ] || rm -f "$PENDING"
            log "skipped: fast-forward to $upstream refused (an untracked file in the way?); left alone"
            exit 0
        fi
        log "pulled ${before:0:12}..${target:0:12} from $upstream"
    elif [ -x "$INSTALLED_BIN" ] && [ ! -e "$PENDING" ]; then
        if [ -e "$HANDOFF_PENDING" ]; then
            log "up to date (${before:0:12}); retrying the handoff that failed last run"
            handoff "$INSTALLED_BIN" || exit 1
            exit 0
        fi
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
    local bin
    if bin="$(command -v bora 2>/dev/null)"; then
        :
    elif [ -x "$INSTALLED_BIN" ]; then
        bin="$INSTALLED_BIN"
    else
        log "failed: release mode but no bora on PATH or at $INSTALLED_BIN"
        exit 1
    fi
    log "release mode ($ROOT is not a git checkout): $bin update --handoff"
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
    # `.git` is a directory in a clone and a file in a linked worktree; tested
    # without git so a missing git cannot turn a checkout into release mode.
    if [ ! -e "$ROOT/.git" ]; then
        release_update
    elif ! command -v git >/dev/null 2>&1; then
        log "failed: $ROOT is a source checkout but git is not on PATH ($PATH); never running bora update over a source build"
        exit 1
    elif ! command -v cargo >/dev/null 2>&1; then
        log "failed: $ROOT is a source checkout but cargo is not on PATH ($PATH); never running bora update over a source build (preview-channel downgrade). Re-run \`just install --only autoupdate\` from a shell with cargo"
        exit 1
    else
        source_update
    fi
}

main "$@"
