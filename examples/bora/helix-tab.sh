#!/bin/sh
# Example script bound to cmd+shift+e in examples/bora/config.toml (the
# `[[keys.command]] key = "cmd+shift+e" command = "bora-helix-tab"` entry).
# Requires Helix (`hx`) installed and on PATH. Put this on PATH as
# `bora-helix-tab` (or point the command at this path directly).
#
# ctrl+alt+a (bora/config.toml): abre — ou foca, se já existir — a tab "helix"
# no workspace focado. Ao criar, roda `hx .` e manda a sequência de duplo
# toque `Space g g` para abrir o picker de arquivos alterados no git assim
# que o Helix carrega. Isso bate com o keymap do autor, que remapeia o
# `Space g` padrão do Helix para duplo toque — se o seu keymap for diferente,
# ajuste a linha `send-keys` abaixo.
set -e
ws=$(bora workspace list | jq -r '.result.workspaces[] | select(.focused).workspace_id')
tab=$(bora tab list --workspace "$ws" | jq -r '[.result.tabs[] | select(.label == "helix")][0].tab_id // empty')
if [ -n "$tab" ]; then
  exec bora tab focus "$tab"
fi
pane=$(bora tab create --workspace "$ws" --label helix --focus | jq -r '.result.root_pane.pane_id')
bora pane run "$pane" hx .
# statusline "NOR" = helix pronto; só então manda Space g g (picker de changed files)
if bora pane wait-output --match NOR --timeout 5000 "$pane" >/dev/null 2>&1; then
  bora pane send-keys "$pane" esc space g g
fi
