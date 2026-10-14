# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=binary
DESCRIPTION="builds release bora and copies it to ~/.local/bin/bora"

available() {
    command -v cargo >/dev/null 2>&1 || { echo "cargo not on PATH"; return 1; }
}

# A copy, not a symlink: this machine pins one shared cargo target dir, so a
# symlink into it is rewritten by any `cargo build --release` from any
# worktree. The target dir comes from `cargo metadata`, which honors both
# CARGO_TARGET_DIR and build.target-dir in ~/.cargo/config.toml. `rm` before
# `install` gives the new binary a new inode, so a running bora keeps its
# old one until `bora server live-handoff`. The version is printed because a
# stale binary looks exactly like a fresh one from outside.
install_component() {
    cd "$ROOT" || return
    cargo build --release --locked
    local target_dir src dest
    target_dir="$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
    src="$target_dir/release/bora"
    dest="$HOME/.local/bin/bora"
    mkdir -p "$HOME/.local/bin"
    if [ -f "$dest" ] && [ ! -L "$dest" ] && cmp -s "$src" "$dest"; then
        say binary "already present ($("$dest" --version))"
        return 0
    fi
    rm -f "$dest"
    command install -m 755 "$src" "$dest"
    say binary "installed ($("$dest" --version))"
}
