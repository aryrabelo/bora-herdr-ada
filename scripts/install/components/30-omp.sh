# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=omp
DESCRIPTION="runs omp plugin install for each entry in scripts/install/omp-plugins.txt (cmd-z-undo, mermaid)"

available() {
    command -v omp >/dev/null 2>&1 || { echo "omp not on PATH"; return 1; }
}

install_component() {
    local installed source name rest field linked failed=0
    installed="$(omp plugin list --json 2>/dev/null || true)"
    while read -r source name rest || [ -n "$source" ]; do
        case "$source" in "" | "#"*) continue ;; esac
        linked=""
        for field in $rest; do
            case "$field" in
            "#"*) break ;;
            linked=*) linked="$HOME/${field#linked=}" ;;
            esac
        done
        if printf '%s' "$installed" | grep -Fq "\"name\": \"$name\"" ||
            { [ -n "$linked" ] && [ -e "$linked" ]; }; then
            say "omp $name" "already present"
        # omp installs user-scope plugins under ~/.omp/plugins.
        elif refuse_symlink "omp $name" "$HOME/.omp/plugins/package.json"; then
            note "install it yourself: omp plugin install $source"
        elif omp plugin install "$source" </dev/null; then
            say "omp $name" "installed"
        else
            say "omp $name" "failed (omp plugin install $source)"
            failed=1
        fi
    done <"$ROOT/scripts/install/omp-plugins.txt"
    return "$failed"
}
