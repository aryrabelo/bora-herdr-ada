#!/usr/bin/env bash
# bora installer: `just install [flags]`.
#
# Components are auto-discovered, in filename order, from
# scripts/install/components/<NN>-<name>.sh. Each file sets NAME and
# DESCRIPTION (one line: what it changes on disk) and defines
# install_component; it may also define `available` (exit non-zero and print
# the reason when it cannot run here) and `default_on` (exit non-zero when it
# is not part of the suggested setup). Both default to yes. Every function
# runs in its own subshell with scripts/install/lib.sh loaded, so components
# cannot leak state into each other.
#
# On a terminal the first question is "Install the suggested setup?": yes =
# every suggested component available here, no = `binary` only (plain fork,
# nothing opinionated), customize = the checklist with suggestions ticked.
# TTY rule: with no flags and stdin or stdout not a terminal, ONLY the
# `binary` component runs, without prompting. That is what
# `just install && bora server live-handoff` relies on from agents and scripts.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="$ROOT/scripts/install"
export ROOT

usage() {
    cat <<'EOF'
Usage: just install [--all | --only <a,b,...> | --yes | --list] [--without <a,b,...>]

With no flags on a terminal: asks "Install the suggested setup? [Y/n/customize]"
  (Y = suggested components, n = binary only, customize = checklist).
With no flags off a terminal (pipe, agent, CI): installs only `binary`.
A selected component that cannot run on this machine fails the install.

  --all            install every component available on this machine
  --only <a,b>     install exactly these components (comma-separated names)
  --yes            install the suggested setup without prompting
  --without <a,b>  drop these components from the selection
  --list           print components and availability, change nothing
  -h, --help       this help
EOF
}

# shellcheck source=install/lib.sh
. "$INSTALL_DIR/lib.sh"

names=() descs=() files=() reasons=() defaults=()
for file in "$INSTALL_DIR"/components/*.sh; do
    [ -e "$file" ] || continue
    meta="$(
        # shellcheck source=/dev/null
        . "$file"
        # shellcheck disable=SC2153  # NAME/DESCRIPTION come from the component
        printf '%s\t%s\n' "$NAME" "$DESCRIPTION"
    )"
    names+=("${meta%%	*}")
    descs+=("${meta#*	}")
    files+=("$file")
    if reason="$(
        # shellcheck source=/dev/null
        . "$file"
        if declare -F available >/dev/null; then available; fi
    )"; then
        reasons+=("")
        if (
            # shellcheck source=/dev/null
            . "$file"
            if declare -F default_on >/dev/null; then default_on; fi
        ); then defaults+=(1); else defaults+=(0); fi
    else
        reasons+=("${reason:-unavailable}")
        defaults+=(0)
    fi
done

count=${#names[@]}
selected=()

index_of() {
    local i
    for ((i = 0; i < count; i++)); do
        if [ "${names[$i]}" = "$1" ]; then
            printf '%s\n' "$i"
            return 0
        fi
    done
    return 1
}

list_components() {
    local i state
    for ((i = 0; i < count; i++)); do
        if [ -n "${reasons[$i]}" ]; then
            state="unavailable (${reasons[$i]})"
        elif [ "${defaults[$i]}" = 1 ]; then
            state="available, suggested"
        else
            state="available, not suggested"
        fi
        printf '%-12s %s\n             %s\n' "${names[$i]}" "${descs[$i]}" "$state"
    done
}

select_binary_only() {
    local i
    for ((i = 0; i < count; i++)); do selected[i]=0; done
    i="$(index_of binary)"
    selected[i]=1
}

# The one explicit choice before anything opinionated is applied.
ask_suggested() {
    local i line
    echo "bora installer. The suggested setup changes, on this machine:"
    for ((i = 0; i < count; i++)); do
        if [ -n "${reasons[$i]}" ]; then
            printf '  -  %-12s unavailable: %s\n' "${names[$i]}" "${reasons[$i]}"
        elif [ "${defaults[$i]}" = 1 ]; then
            printf '  +  %-12s %s\n' "${names[$i]}" "${descs[$i]}"
        fi
    done
    while true; do
        printf 'Install the suggested setup? [Y/n/customize] (n = binary only): '
        if ! IFS= read -r line; then
            echo
            exit 1
        fi
        case "$line" in
            "" | y | Y | yes) selected=("${defaults[@]}"); return 0 ;;
            n | N | no) select_binary_only; return 0 ;;
            c | C | customize) menu; return 0 ;;
            *) echo "  answer y, n or customize" ;;
        esac
    done
}

menu() {
    local i mark line tok
    selected=("${defaults[@]}")
    while true; do
        echo
        echo "Select what to install (suggested ticked):"
        for ((i = 0; i < count; i++)); do
            if [ -n "${reasons[$i]}" ]; then
                mark="-"
            elif [ "${selected[$i]}" = 1 ]; then
                mark="x"
            else
                mark=" "
            fi
            printf '  %d) [%s] %-12s %s' "$((i + 1))" "$mark" "${names[$i]}" "${descs[$i]}"
            if [ -n "${reasons[$i]}" ]; then printf ' (unavailable: %s)' "${reasons[$i]}"; fi
            printf '\n'
        done
        printf 'Toggle by number (e.g. "2 4"), a = all, n = none, Enter = install, q = quit: '
        if ! IFS= read -r line; then
            echo
            exit 1
        fi
        case "$line" in
            "") return 0 ;;
            q | Q) echo "nothing installed"; exit 0 ;;
            a | A)
                for ((i = 0; i < count; i++)); do
                    if [ -z "${reasons[$i]}" ]; then selected[i]=1; fi
                done
                continue
                ;;
            n | N)
                for ((i = 0; i < count; i++)); do selected[i]=0; done
                continue
                ;;
        esac
        for tok in ${line//,/ }; do
            if ! [[ "$tok" =~ ^[0-9]+$ ]] || ((tok < 1 || tok > count)); then
                echo "  ignored: $tok (not a component number)"
                continue
            fi
            i=$((tok - 1))
            if [ -n "${reasons[$i]}" ]; then
                echo "  ${names[$i]} is unavailable: ${reasons[$i]}"
            elif [ "${selected[$i]}" = 1 ]; then
                selected[i]=0
            else
                selected[i]=1
            fi
        done
    done
}

mode="" only="" without=""
while [ $# -gt 0 ]; do
    case "$1" in
        --all) [ -z "$mode" ] || { usage >&2; exit 2; }; mode=all ;;
        --yes | -y) [ -z "$mode" ] || [ "$mode" = yes ] || { usage >&2; exit 2; }; mode=yes ;;
        --list) mode=list ;;
        --only)
            [ -z "$mode" ] && [ $# -ge 2 ] || { usage >&2; exit 2; }
            mode=only only="$2"
            shift
            ;;
        --only=*) [ -z "$mode" ] || { usage >&2; exit 2; }; mode=only only="${1#--only=}" ;;
        --without)
            [ $# -ge 2 ] || { usage >&2; exit 2; }
            without="$2"
            shift
            ;;
        --without=*) without="${1#--without=}" ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

case "$mode" in
    list)
        list_components
        exit 0
        ;;
    all)
        for ((i = 0; i < count; i++)); do
            if [ -z "${reasons[$i]}" ]; then selected[i]=1; else selected[i]=0; fi
        done
        ;;
    yes)
        selected=("${defaults[@]}")
        ;;
    only)
        for ((i = 0; i < count; i++)); do selected[i]=0; done
        for name in ${only//,/ }; do
            if ! i="$(index_of "$name")"; then
                echo "unknown component: $name (known: ${names[*]})" >&2
                exit 2
            fi
            selected[i]=1
        done
        ;;
    "")
        if [ -t 0 ] && [ -t 1 ]; then
            ask_suggested
        else
            echo "not a terminal: installing only 'binary' (see --help for --all/--only/--yes)" >&2
            select_binary_only
        fi
        ;;
esac

for name in ${without//,/ }; do
    if ! i="$(index_of "$name")"; then
        echo "unknown component: $name (known: ${names[*]})" >&2
        exit 2
    fi
    selected[i]=0
done

# A selected component that cannot run here FAILS: the selection was explicit
# (--only, the menu) or implied (binary off a terminal, `n`), and a skip would
# let `just install && bora server live-handoff` hand off to a stale binary.
failed=()
for ((i = 0; i < count; i++)); do
    [ "${selected[$i]:-0}" = 1 ] || continue
    if [ -n "${reasons[$i]}" ]; then
        say "${names[$i]}" "failed (${reasons[$i]})"
        failed+=("${names[$i]}")
        continue
    fi
    # Not under `if`: errexit is ignored there, and a failing command must
    # stop its component.
    set +e
    (
        set -e
        # shellcheck source=/dev/null
        . "${files[$i]}"
        install_component
    )
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then
        say "${names[$i]}" "failed (exit $rc, see output above)"
        failed+=("${names[$i]}")
    fi
done

if [ ${#failed[@]} -gt 0 ]; then
    echo "failed components: ${failed[*]}" >&2
    exit 1
fi
