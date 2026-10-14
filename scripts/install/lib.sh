# shellcheck shell=bash
# Shared helpers for scripts/install.sh and its components. Sourced, never
# executed. Every helper here is available inside a component's functions.

# say THING STATUS — the one status line format every component prints:
#   <thing>: installed | already present | skipped (<reason>) | failed (<reason>)
say() {
    printf '%s: %s\n' "$1" "$2"
}

# note TEXT — an indented follow-up line under the last status line.
note() {
    printf '    %s\n' "$1"
}

is_macos() {
    [ "$(uname -s)" = "Darwin" ]
}

# Where a path really lives, through every symlink (for notices only; never
# written through). Missing components resolve via the nearest existing
# ancestor.
resolve_link() {
    local p="$1" rest=""
    while [ ! -e "$p" ] && [ ! -L "$p" ] && [ "$p" != "/" ] && [ "$p" != "." ]; do
        rest="/$(basename "$p")$rest"
        p="$(dirname "$p")"
    done
    if [ -d "$p" ] && [ ! -L "$p" ]; then
        printf '%s%s\n' "$(cd -P "$p" && pwd -P)" "$rest"
    else
        printf '%s%s\n' "$(readlink -f "$p" 2>/dev/null || printf '%s' "$p")" "$rest"
    fi
}

# bora's namespace directory name, as app_dir_name() in src/config/io.rs
# resolves it for a release build: trimmed $HERDR_NAMESPACE, else "bora".
bora_namespace() {
    local ns="${HERDR_NAMESPACE:-}"
    ns="${ns#"${ns%%[![:space:]]*}"}"
    ns="${ns%"${ns##*[![:space:]]}"}"
    printf '%s\n' "${ns:-bora}"
}

# bora's state directory on Unix: $XDG_STATE_HOME, else ~/.local/state.
bora_state_dir() {
    printf '%s/%s\n' "${XDG_STATE_HOME:-$HOME/.local/state}" "$(bora_namespace)"
}

# symlinked_component PATH — the write guard. Prints the first component of
# PATH that is a symlink (PATH itself, or any parent directory) and succeeds;
# fails when there is none. Run it before every write or mkdir: a symlinked
# ~/.config, ~/.config/ghostty or config file usually points into a dotfiles
# repo, and writing through it edits that repo. $HOME and its ancestors are
# not checked (system layout, not dotfiles). Components that do not exist yet
# are fine: mkdir creates real directories.
symlinked_component() {
    local p="$1" home="${HOME%/}"
    case "$p" in /*) ;; *) p="$PWD/$p" ;; esac
    while [ -n "$p" ] && [ "$p" != "/" ]; do
        p="${p%/}"
        case "$home/" in "$p"/*) return 1 ;; esac
        if [ -L "$p" ]; then
            printf '%s\n' "$p"
            return 0
        fi
        p="$(dirname "$p")"
    done
    return 1
}

# refuse_symlink THING PATH — say/notes for a write refused by the guard.
refuse_symlink() {
    local link
    link="$(symlinked_component "$2")" || return 1
    say "$1" "skipped ($link is a symlink to $(resolve_link "$link"); not writing through it)"
}

# The bora binary to drive: the one on PATH, else the one the `binary`
# component installs.
bora_bin() {
    if command -v bora >/dev/null 2>&1; then
        command -v bora
    elif [ -x "$HOME/.local/bin/bora" ]; then
        printf '%s\n' "$HOME/.local/bin/bora"
    else
        return 1
    fi
}

# bora's config directory, resolved like config_dir() in src/config/io.rs:
# $XDG_CONFIG_HOME, else ~/.config, joined with the namespace.
bora_config_dir() {
    printf '%s/%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}" "$(bora_namespace)"
}

# bora's config.toml, honoring HERDR_CONFIG_PATH like config_path() does.
bora_config_file() {
    if [ -n "${HERDR_CONFIG_PATH:-}" ]; then
        printf '%s\n' "$HERDR_CONFIG_PATH"
    else
        printf '%s/config.toml\n' "$(bora_config_dir)"
    fi
}

ghostty_config_dir() {
    printf '%s/ghostty\n' "${XDG_CONFIG_HOME:-$HOME/.config}"
}
