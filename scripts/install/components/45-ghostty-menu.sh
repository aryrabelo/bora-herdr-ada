# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=ghostty-menu
DESCRIPTION="changes macOS user defaults for Ghostty (NSUserKeyEquivalents): moves the Show Next/Previous Tab and Reload Configuration menu shortcuts to cmd+ctrl so cmd+shift+] [ , reach bora; quit (Cmd+Q) and relaunch Ghostty"

# macOS resolves application-menu shortcuts before Ghostty's keybind table, so
# `keybind = cmd+shift+]=unbind` cannot free a chord a menu item owns. The
# items keep working on the cmd+ctrl level ghostty.conf gives Ghostty.
# NSUserKeyEquivalents keys are the menu titles AS SHOWN, i.e. localized, and a
# wrong title is ignored silently; ghostty-menu-titles.js resolves them for
# this Mac's languages and Ghostty's bundle. Glyphs: @ cmd, ^ ctrl, $ shift.
# Test overrides: BORA_INSTALL_GHOSTTY_DEFAULTS_DOMAIN (the domain written),
# BORA_INSTALL_APPLE_LANGUAGES (comma list replacing AppleLanguages) and
# BORA_INSTALL_GHOSTTY_APP (the Ghostty.app whose localizations count).
GHOSTTY_MENU_DOMAIN="${BORA_INSTALL_GHOSTTY_DEFAULTS_DOMAIN:-com.mitchellh.ghostty}"
GHOSTTY_MENU_ITEMS=(
    "Show Next Tab=@^]"
    "Show Previous Tab=@^["
    "Reload Configuration=@^\$,"
)

ghostty_app() {
    local bin
    if [ -n "${BORA_INSTALL_GHOSTTY_APP:-}" ]; then
        printf '%s\n' "$BORA_INSTALL_GHOSTTY_APP"
    elif [ -d /Applications/Ghostty.app ]; then
        printf '%s\n' /Applications/Ghostty.app
    elif [ -d "$HOME/Applications/Ghostty.app" ]; then
        printf '%s\n' "$HOME/Applications/Ghostty.app"
    elif bin="$(command -v ghostty 2>/dev/null)"; then
        bin="$(resolve_link "$bin")"
        case "$bin" in
            */Contents/MacOS/*) printf '%s\n' "${bin%/Contents/MacOS/*}" ;;
            *) return 1 ;;
        esac
    else
        return 1
    fi
}

available() {
    is_macos || { echo "macOS only"; return 1; }
    local tool
    for tool in defaults plutil osascript; do
        command -v "$tool" >/dev/null 2>&1 || { echo "$tool not found"; return 1; }
    done
    ghostty_app >/dev/null || { echo "Ghostty.app not found"; return 1; }
}

# Current value of one menu item's shortcut, empty when unset. `defaults
# export` goes through cfprefsd, so it sees values not yet flushed to disk.
ghostty_menu_value() {
    defaults export "$GHOSTTY_MENU_DOMAIN" - 2>/dev/null |
        plutil -extract "NSUserKeyEquivalents.$1" raw -o - - 2>/dev/null || true
}

install_component() {
    local resolved line title value old i changed=0 tab=$'\t'
    local -a titles=()
    resolved="$(osascript -l JavaScript "$ROOT/scripts/install/ghostty-menu-titles.js" \
        "$(ghostty_app)" "${BORA_INSTALL_APPLE_LANGUAGES:-}" com.mitchellh.ghostty)"
    case "$resolved" in
        ERROR*)
            # Never claim "installed" under a title the menu does not show.
            say "ghostty menu" "failed (${resolved#ERROR"$tab"})"
            return 1
            ;;
    esac
    note "menu titles for $(sed -n "s/^LANG$tab//p" <<<"$resolved")"
    # One localized title per item, looked up by its English title.
    for ((i = 0; i < ${#GHOSTTY_MENU_ITEMS[@]}; i++)); do
        line="$(grep -F "${GHOSTTY_MENU_ITEMS[$i]%%=*}$tab" <<<"$resolved" || true)"
        if [ -z "$line" ]; then
            say "ghostty menu" "failed (no title resolved for \"${GHOSTTY_MENU_ITEMS[$i]%%=*}\")"
            return 1
        fi
        titles+=("${line#*"$tab"}")
    done
    if refuse_symlink "ghostty menu" "$HOME/Library/Preferences/$GHOSTTY_MENU_DOMAIN.plist"; then
        for ((i = 0; i < ${#titles[@]}; i++)); do
            note "defaults write $GHOSTTY_MENU_DOMAIN NSUserKeyEquivalents -dict-add '${titles[$i]}' '\"${GHOSTTY_MENU_ITEMS[$i]#*=}\"'"
        done
        return 0
    fi
    for ((i = 0; i < ${#titles[@]}; i++)); do
        title="${titles[$i]}"
        value="${GHOSTTY_MENU_ITEMS[$i]#*=}"
        old="$(ghostty_menu_value "$title")"
        if [ "$old" = "$value" ]; then
            say "ghostty menu \"$title\"" "already present ($value)"
            continue
        fi
        if [ -n "$old" ]; then
            note "ghostty menu \"$title\": replacing $old -> $value"
        fi
        # defaults parses the value as an old-style plist; unquoted,
        # "@^]" is rejected ("Could not parse"), so pass a quoted string.
        defaults write "$GHOSTTY_MENU_DOMAIN" NSUserKeyEquivalents -dict-add "$title" "\"$value\""
        say "ghostty menu \"$title\"" "installed ($value in $GHOSTTY_MENU_DOMAIN)"
        changed=1
    done
    if [ "$changed" = 1 ]; then
        note "quit Ghostty (Cmd+Q) and relaunch; a config reload does not rebuild the menu"
    fi
}
