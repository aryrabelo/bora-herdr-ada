#!/usr/bin/env bash
# The gitui tab actions and event hooks (herdr-plugin.toml).
#
#   tab.sh toggle      open the gitui tab, or close it if it's already open
#   tab.sh open        open the gitui tab, no-op (focuses it) if already open
#   tab.sh close       close the gitui tab, no-op if none open
#   tab.sh auto-open   worktree.created/worktree.opened hook: open, no focus steal
#
# The tab is identified by its label ("gitui" — the manifest pane's `title`,
# applied by bora's plugin-tab-title fix). No process probing, no config
# resolution: this plugin has exactly one fixed-placement pane and no
# settings, so unlike a more general plugin there's nothing to negotiate.
set -uo pipefail

# bora runs plugin commands with a minimal PATH; prepend the common install
# locations so jq/git/gitui resolve regardless of shell profile.
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${PATH:-}"

mode="${1:-toggle}"
H="${HERDR_BIN_PATH:-bora}"

refuse() {
  echo "gitui plugin: $1" >&2
  exit 1
}

# auto-open fires with no focused pane; target the fresh/switched-to
# workspace from the event payload instead of HERDR_WORKSPACE_ID.
if [ "$mode" = auto-open ]; then
  [ -n "${HERDR_PLUGIN_EVENT_JSON:-}" ] || refuse "no event payload for auto-open"
  ws=$(printf '%s' "$HERDR_PLUGIN_EVENT_JSON" | jq -r '.data.workspace.workspace_id // empty' 2>/dev/null)
  cwd=$(printf '%s' "$HERDR_PLUGIN_EVENT_JSON" | jq -r '.data.workspace.worktree.checkout_path // empty' 2>/dev/null)
else
  ws="${HERDR_WORKSPACE_ID:-}"
  cwd=""
  [ -n "${HERDR_PLUGIN_CONTEXT_JSON:-}" ] &&
    cwd=$(printf '%s' "$HERDR_PLUGIN_CONTEXT_JSON" | jq -r '.focused_pane_cwd // .workspace_cwd // empty' 2>/dev/null)
fi
[ -n "$ws" ] || refuse "no workspace context (invoke from inside bora)"

existing=$("$H" tab list --workspace "$ws" 2>/dev/null | jq -r '[.result.tabs[] | select(.label == "gitui")][0].tab_id // empty')

case "$mode" in
close)
  [ -n "$existing" ] || { printf 'close: nothing open in %s\n' "$ws"; exit 0; }
  "$H" tab close "$existing" >/dev/null || refuse "bora tab close failed for $existing"
  printf 'closed %s in %s\n' "$existing" "$ws"
  exit 0
  ;;
toggle)
  if [ -n "$existing" ]; then
    "$H" tab close "$existing" >/dev/null || refuse "bora tab close failed for $existing"
    printf 'closed %s in %s\n' "$existing" "$ws"
    exit 0
  fi
  ;;
open | auto-open)
  if [ -n "$existing" ]; then
    [ "$mode" = open ] && "$H" tab focus "$existing" >/dev/null
    exit 0
  fi
  ;;
*)
  refuse "unknown mode '$mode' (toggle | open | close | auto-open)"
  ;;
esac

# Opening from here on. Only inside a git repo, and only if gitui is
# actually installed — a missing gitui would otherwise open a tab that
# immediately exits.
[ -n "$cwd" ] && git -C "$cwd" rev-parse --show-toplevel >/dev/null 2>&1 ||
  refuse "not a git repo: '${cwd:-<no cwd>}'"
command -v gitui >/dev/null 2>&1 || refuse "gitui not found on PATH"

focus=--no-focus
[ "$mode" != auto-open ] && focus=--focus

open_json=$("$H" plugin pane open --plugin "${HERDR_PLUGIN_ID:-ary.gitui}" --entrypoint tab \
  --workspace "$ws" --cwd "$cwd" "$focus" 2>/dev/null)
new=$(printf '%s' "$open_json" | jq -r '.result.plugin_pane.pane.pane_id // empty' 2>/dev/null)
[ -n "$new" ] || refuse "bora plugin pane open failed"

# No rename here: `open_plugin_tab` already applies the manifest pane's
# `title` ("gitui") as the tab's custom name (0.27.0+), so the tab reads
# right without a follow-up `bora tab rename` call.
[ "$mode" = auto-open ] || printf 'opened %s in %s\n' "$new" "$ws"
