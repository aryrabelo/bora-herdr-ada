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

# Where a symlink finally points (for notices only; never written through).
resolve_link() {
    readlink -f "$1" 2>/dev/null || readlink "$1"
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

# bora's config directory, resolved exactly like config_dir() and
# app_dir_name() in src/config/io.rs (release build): $XDG_CONFIG_HOME, else
# ~/.config, joined with the trimmed $HERDR_NAMESPACE, else "bora".
bora_config_dir() {
    local ns="${HERDR_NAMESPACE:-}"
    ns="${ns#"${ns%%[![:space:]]*}"}"
    ns="${ns%"${ns##*[![:space:]]}"}"
    printf '%s/%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}" "${ns:-bora}"
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
