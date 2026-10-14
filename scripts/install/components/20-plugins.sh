# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=plugins
DESCRIPTION="installs the bora plugins in scripts/install/plugins.txt (file viewer, tab auto-rename) under ~/.config/bora/plugins"

# Always offered: on a fresh machine bora appears only once `binary` (which
# runs first) has installed it, so the check happens at install time, and a
# missing bora fails the component.
install_component() {
    local bin registry source id rest guarded="" path failed=0
    if ! bin="$(bora_bin)"; then
        note "bora is not on PATH or in ~/.local/bin; select binary too"
        return 1
    fi
    registry="$(bora_config_dir)/plugins.json"
    # `bora plugin install` writes the registry, the plugin checkout under
    # <config>/plugins and logs under the state dir.
    for path in "$registry" "$(bora_config_dir)/plugins" "$(bora_state_dir)"; do
        if guarded="$(symlinked_component "$path")"; then break; fi
    done
    while read -r source id rest || [ -n "$source" ]; do
        case "$source" in "" | "#"*) continue ;; esac
        if [ -z "$id" ] || [ "${id#\#}" != "$id" ]; then
            id="${source#*/}"
            id="${id%%/*}"
        fi
        # plugins.json is serde_json pretty output: `"plugin_id": "<id>"`.
        if [ -f "$registry" ] && grep -Fq "\"plugin_id\": \"$id\"" "$registry"; then
            say "plugin $id" "already present"
        elif [ -n "$guarded" ]; then
            say "plugin $id" "skipped ($guarded is a symlink to $(resolve_link "$guarded"); not writing through it)"
            note "install it yourself: bora plugin install $source --yes"
        elif "$bin" plugin install "$source" --yes </dev/null; then
            say "plugin $id" "installed"
        else
            say "plugin $id" "failed (bora plugin install $source --yes)"
            failed=1
        fi
    done <"$ROOT/scripts/install/plugins.txt"
    return "$failed"
}
