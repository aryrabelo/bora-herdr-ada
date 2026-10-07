# Fork example plugins — `examples/bora/plugins/`

Fork-owned bora plugins with no upstream counterpart. Upstream never touches this
directory, so nothing here can conflict on an upstream sync. Each subdirectory is
one self-contained plugin, installed with
`bora plugin link examples/bora/plugins/<name>` (link does not run `[[build]]` and
does not run `[[startup]]`).

|Dir|Plugin id|Does|
|---|---|---|
|`gitui/`|`ary.gitui`|gitui in its own tab per worktree|
|`pane-timer/`|`ary.pane-timer`|timer that pins a pane to `blocked` (red) or `done` after a delay|
|`auto-group/`|`ary.auto-group`|rules file that puts each workspace in a sidebar group by repository|
|`windhover-push/`|`ary.windhover-push`|encrypted iPhone push (Windhover) when an agent is blocked, finishes, or asks the human|

Prefer a plugin here over a core patch whenever the plugin API can express the
feature; the repo root `AGENTS.md` (prior-art and fork-merge-friction rules) says why.

## Contract

- **Logic plugins are Python 3.9, stdlib only.** bora runs plugin commands with a
  minimal `PATH`, where `python3` can be `/usr/bin/python3` (3.9.6): no `tomllib`,
  no `match`, no runtime `X | Y` unions, no third-party imports. Manifest commands
  are argv arrays (no shell): `command = ["python3", "x.py", ...]`.
  **One exception, `windhover-push/`:** its AES-256-GCM envelope must match
  Windhover's WebCrypto vectors, and the 3.9 stdlib has no AES-GCM. It is plain
  ESM JavaScript on WebCrypto with no npm dependencies, launched as
  `["sh", "run.sh", ...]`, which picks `bun` or `node` >= 20 from `PATH` or the
  usual install paths. Any other plugin stays Python.
- **Each plugin carries its own `bora_api.py` (`bora_api.mjs` in `windhover-push/`); nothing is shared across plugin
  directories.** The client speaks NDJSON to the Unix socket and reads the path
  ONLY from `HERDR_SOCKET_PATH`, with no default and no guessing. A shell inside a
  bora pane inherits the LIVE server's `HERDR_SOCKET_PATH`, so any fallback lets a
  test or a manual run mutate the operator's real sessions.
- **Methods the CLI has no verb for go through the socket, not through new core
  CLI verbs:** `pane.set_status`, `workspace.set_group`, and `plugin.pane.open`
  with `placement = "popup"` (the CLI's `--placement` enum cannot say `popup`).
- **`[[startup]]` runs at server start and again on live handoff, never on
  `plugin link`.** A daemon plugin must therefore hold a single-instance `flock`
  (the handoff spawns a second one) and start itself on first use instead of
  waiting for the next server start. Never keep durable state under the plugin
  root; use `HERDR_PLUGIN_STATE_DIR` / `HERDR_PLUGIN_CONFIG_DIR`.
- **A plugin never overrides a choice the operator made by hand** (a group set
  from the menu, a status pin cleared with `Auto`). Automation acts on creation
  events or on an explicit command; forcing is CLI-only, never bound to a hook or
  action.

## Testing

- Unit tests: `python3 -m unittest discover -s <plugin>/tests`, run under BOTH
  `/usr/bin/python3` and the default `python3`, with every `HERDR_*` variable
  unset. `windhover-push/`: `node --test tests/*.test.mjs` under Node 20 and the
  default `node`, plus `bun test tests/`, with every `HERDR_*` variable unset.
- End to end: an isolated namespace server, per the root `AGENTS.md`
  "Trialling a third-party plugin" rule, with the whole scrub: `env -u
  HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH -u HERDR_ENV -u HERDR_PANE_ID -u
  HERDR_WORKSPACE_ID -u HERDR_TAB_ID -u HERDR_BIN_PATH HOME=… HERDR_NAMESPACE=…
  XDG_CONFIG_HOME=… XDG_STATE_HOME=… XDG_DATA_HOME=…`. Prove the run stayed off the
  live server: `shasum -a 256 ~/.config/bora/plugins.json` identical before and
  after.
- A headless server can open a popup (`plugin.pane.open` succeeds, a second open
  reports `ui_busy`) but nothing renders it; the popup UI needs a real client.
