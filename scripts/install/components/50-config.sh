# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=config
DESCRIPTION="creates ~/.config/bora/config.toml from examples/opinionated/config.toml if absent; never edits an existing one"

# Prints the template's settings the user's config does not mention. A plain
# grep on key names and [[keys.command]] commands: enough to show what the
# template would add, not a TOML diff.
config_missing_keys() {
    local user="$1" template="$2" key value missing=()
    while IFS= read -r key; do
        grep -Eq "^[[:space:]]*${key}[[:space:]]*=" "$user" || missing+=("$key")
    done < <(sed -n 's/^\([a-z_]*\)[[:space:]]*=.*/\1/p' "$template" | grep -Ev '^(key|type|command|description)$' | sort -u)
    while IFS= read -r value; do
        grep -Fq "\"$value\"" "$user" || missing+=("command $value")
    done < <(sed -n 's/^command[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$template")
    if [ ${#missing[@]} -gt 0 ]; then
        local joined
        joined="$(printf '%s, ' "${missing[@]}")"
        note "lacks template keys: ${joined%, }"
        note "template: $template"
    fi
}

install_component() {
    local cfg template
    cfg="$(bora_config_file)"
    template="$ROOT/examples/opinionated/config.toml"
    if [ -e "$cfg" ] && [ ! -L "$cfg" ]; then
        say "bora config.toml" "already present ($cfg)"
        config_missing_keys "$cfg" "$template"
    elif refuse_symlink "bora config.toml" "$cfg"; then
        if [ -r "$cfg" ]; then
            config_missing_keys "$cfg" "$template"
        else
            note "to adopt the template, copy $template to $(resolve_link "$cfg") yourself"
        fi
    else
        mkdir -p "$(dirname "$cfg")"
        cp "$template" "$cfg"
        say "bora config.toml" "installed ($cfg)"
    fi
}
