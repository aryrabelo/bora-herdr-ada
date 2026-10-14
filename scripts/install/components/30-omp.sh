# shellcheck shell=bash
# shellcheck disable=SC2034  # NAME/DESCRIPTION are read by scripts/install.sh
NAME=omp
DESCRIPTION="runs omp plugin install github:aryrabelo/omp-cmd-z-undo (Ctrl+Z / Cmd+Z undo typing in omp)"

OMP_UNDO_SOURCE="github:aryrabelo/omp-cmd-z-undo"

available() {
    command -v omp >/dev/null 2>&1 || { echo "omp not on PATH"; return 1; }
}

# Already present when omp lists the package, or when the extension file is
# linked by hand (the README's alternative install), so it never loads twice.
install_component() {
    if omp plugin list --json 2>/dev/null | grep -q 'omp-cmd-z-undo' ||
        [ -e "$HOME/.omp/agent/extensions/cmd-z-undo.ts" ]; then
        say "omp cmd-z-undo" "already present"
        return 0
    fi
    # omp installs user-scope plugins under ~/.omp/plugins.
    if refuse_symlink "omp cmd-z-undo" "$HOME/.omp/plugins/package.json"; then
        note "install it yourself: omp plugin install $OMP_UNDO_SOURCE"
        return 0
    fi
    omp plugin install "$OMP_UNDO_SOURCE"
    say "omp cmd-z-undo" "installed"
}
