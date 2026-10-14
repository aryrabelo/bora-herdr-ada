# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=ghostty
DESCRIPTION="writes ~/.config/ghostty/bora.conf (Cmd chords to bora, Option as Alt, Cmd+Z undo) and adds one include line to your Ghostty config"

# The layer is macOS-specific (cmd chords, macos-* keys), so it is not offered
# elsewhere even if Ghostty is installed.
available() {
    is_macos || { echo "macOS only"; return 1; }
    command -v ghostty >/dev/null 2>&1 ||
        [ -d /Applications/Ghostty.app ] ||
        [ -d "$HOME/Applications/Ghostty.app" ] ||
        { echo "Ghostty not installed"; return 1; }
}

# Relative include: Ghostty resolves it against the directory of the file that
# contains it (also when that file is a symlink), i.e. next to bora.conf. `?`
# makes a missing bora.conf a no-op instead of a config error.
GHOSTTY_INCLUDE='config-file = ?bora.conf'

ghostty_has_include() {
    [ -r "$1" ] && grep -Eq '^[[:space:]]*config-file[[:space:]]*=[[:space:]]*"?\??[^#]*bora\.conf' "$1"
}

install_component() {
    local dir managed template user="" candidate
    dir="$(ghostty_config_dir)"
    managed="$dir/bora.conf"
    template="$ROOT/examples/opinionated/ghostty.conf"

    # bora.conf is ours and rewritten on every install, unless it or a parent
    # directory is a symlink: then it lives in someone's dotfiles.
    if refuse_symlink "ghostty bora.conf" "$managed"; then
        note "to adopt the template, copy $template to $(resolve_link "$managed") yourself"
    elif [ -f "$managed" ] && cmp -s "$template" "$managed"; then
        say "ghostty bora.conf" "already present"
    else
        mkdir -p "$dir"
        cp "$template" "$managed.tmp.$$"
        mv -f "$managed.tmp.$$" "$managed"
        say "ghostty bora.conf" "installed ($managed)"
    fi

    # Ghostty loads both `config` and `config.ghostty` (in that order); the
    # include counts in either. Append to config.ghostty first, else config.
    for candidate in "$dir/config.ghostty" "$dir/config"; do
        if [ -e "$candidate" ] || [ -L "$candidate" ]; then
            if ghostty_has_include "$candidate"; then
                say "ghostty include" "already present ($candidate)"
                return 0
            fi
            [ -n "$user" ] || user="$candidate"
        fi
    done
    [ -n "$user" ] || user="$dir/config.ghostty"

    if refuse_symlink "ghostty include" "$user"; then
        note "add this line to $(resolve_link "$user"):"
        note "  $GHOSTTY_INCLUDE"
        return 0
    elif [ -e "$user" ]; then
        printf '\n# bora opinionated layer, managed by just install\n%s\n' "$GHOSTTY_INCLUDE" >>"$user"
        say "ghostty include" "installed (appended to $user)"
    else
        mkdir -p "$dir"
        printf '# bora opinionated layer, managed by just install\n%s\n' "$GHOSTTY_INCLUDE" >"$user"
        say "ghostty include" "installed (created $user)"
    fi
    note "reload Ghostty (menu Ghostty > Reload Configuration); macos-* keys need a full quit and relaunch"
}
