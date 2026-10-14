# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=ghostty-menu
DESCRIPTION="changes macOS user defaults for Ghostty (NSUserKeyEquivalents): moves the Show Next/Previous Tab and Reload Configuration menu shortcuts to cmd+ctrl so cmd+shift+] [ , reach bora; quit (Cmd+Q) and relaunch Ghostty"

# macOS resolves application-menu shortcuts before Ghostty's keybind table, so
# `keybind = cmd+shift+]=unbind` cannot free a chord a menu item owns. The
# items keep working on the cmd+ctrl level ghostty.conf gives Ghostty.
# Titles are the exact menu item titles (read from the accessibility tree); a
# wrong title is ignored silently. Glyphs: @ cmd, ^ ctrl, $ shift, ~ alt.
# BORA_INSTALL_GHOSTTY_DEFAULTS_DOMAIN overrides the domain (for tests).
GHOSTTY_MENU_DOMAIN="${BORA_INSTALL_GHOSTTY_DEFAULTS_DOMAIN:-com.mitchellh.ghostty}"
GHOSTTY_MENU_ITEMS=(
    "Show Next Tab=@^]"
    "Show Previous Tab=@^["
    "Reload Configuration=@^\$,"
)

available() {
    is_macos || { echo "macOS only"; return 1; }
    if ! command -v defaults >/dev/null 2>&1 || ! command -v plutil >/dev/null 2>&1; then
        echo "defaults/plutil not found"
        return 1
    fi
    command -v ghostty >/dev/null 2>&1 ||
        [ -d /Applications/Ghostty.app ] ||
        [ -d "$HOME/Applications/Ghostty.app" ] ||
        { echo "Ghostty not installed"; return 1; }
}

# Current value of one menu item's shortcut, empty when unset. `defaults
# export` goes through cfprefsd, so it sees values not yet flushed to disk.
ghostty_menu_value() {
    defaults export "$GHOSTTY_MENU_DOMAIN" - 2>/dev/null |
        plutil -extract "NSUserKeyEquivalents.$1" raw -o - - 2>/dev/null || true
}

install_component() {
    local item title value changed=0
    if refuse_symlink "ghostty menu" "$HOME/Library/Preferences/$GHOSTTY_MENU_DOMAIN.plist"; then
        for item in "${GHOSTTY_MENU_ITEMS[@]}"; do
            note "defaults write $GHOSTTY_MENU_DOMAIN NSUserKeyEquivalents -dict-add '${item%%=*}' '\"${item#*=}\"'"
        done
        return 0
    fi
    for item in "${GHOSTTY_MENU_ITEMS[@]}"; do
        title="${item%%=*}"
        value="${item#*=}"
        if [ "$(ghostty_menu_value "$title")" = "$value" ]; then
            say "ghostty menu \"$title\"" "already present ($value)"
        else
            # defaults parses the value as an old-style plist; unquoted,
            # "@^]" is rejected ("Could not parse"), so pass a quoted string.
            defaults write "$GHOSTTY_MENU_DOMAIN" NSUserKeyEquivalents -dict-add "$title" "\"$value\""
            say "ghostty menu \"$title\"" "installed ($value in $GHOSTTY_MENU_DOMAIN)"
            changed=1
        fi
    done
    if [ "$changed" = 1 ]; then
        note "quit Ghostty (Cmd+Q) and relaunch; a config reload does not rebuild the menu"
    fi
}
