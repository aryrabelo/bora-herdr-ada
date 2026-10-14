# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=binary
DESCRIPTION="builds release bora, copies it to ~/.local/bin/bora and links ~/.local/bin/herdr to it"

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
    else
        rm -f "$dest"
        command install -m 755 "$src" "$dest"
        say binary "installed ($("$dest" --version))"
    fi
    link_herdr_alias
}

# Plugins written for upstream call the binary by its upstream name (gh-pr
# runs `herdr pane current` and swallows the failure), so without this alias
# they install and silently do nothing. An existing herdr is never replaced:
# it may be a real upstream install.
link_herdr_alias() {
    local alias="$HOME/.local/bin/herdr" target
    target="$(readlink "$alias" 2>/dev/null || true)"
    if [ "$target" = bora ] || [ "$target" = "$HOME/.local/bin/bora" ]; then
        say "herdr alias" "already present"
    elif [ -e "$alias" ] || [ -L "$alias" ]; then
        say "herdr alias" "skipped ($alias exists and is not a link to bora)"
        note "plugins that call \`herdr\` will reach that binary, not bora"
    else
        ln -s bora "$alias"
        say "herdr alias" "installed ($alias -> bora)"
    fi
}
