# ary.pane-timer

Arm a timer on a pane. When the delay elapses the pane's sidebar status is
pinned to `blocked` (drawn red) or `done` (drawn teal), through the socket
method `pane.set_status`. No core changes: it is a plain plugin (Python 3.9+,
standard library only).

The pin stays until you clear it yourself (pane context menu → `Status: Auto`);
the plugin never clears it.

## Install and first use

```sh
bora plugin link examples/bora/plugins/pane-timer
```

Then paste the key binding below into `config.toml`. That is
all: the first timer you arm (popup or `timer.py add`) starts the timer daemon
if none is running. Nothing needs a server restart. On later server starts the
plugin's `[[startup]]` hook starts the daemon.

**Upgrading the plugin code:** a running daemon keeps the old code. Run
`python3 examples/bora/plugins/pane-timer/timer.py stop`; the next `add` (or the
next server start) respawns it from the new code. A live handoff does *not*
replace it (see Limits).

## Key binding

This snippet is what was tested in a real bora client (`prefix+t` is only
"free" in the default keymap; pick another key if you already use it):

```toml
[[keys.command]]
key = "prefix+t"
type = "plugin_action"
command = "ary.pane-timer.set"
description = "pane timer"
```

With the default prefix, press `ctrl+b`, then `t`. A small popup opens for the
focused pane: it asks for the delay (`10m`, `1h30m`, bare number = minutes,
empty = cancel), then the status (`b` = blocked/red, the default, or `d` =
done). `ary.pane-timer.cancel` cancels the focused pane's timer; bind it the
same way if you want a key for it.

## CLI

Run from a bora pane (`HERDR_SOCKET_PATH` is set there) with `python3 timer.py`:

| command | what it does |
| --- | --- |
| `add <pane_id> <delay> <status>` | arm the pane (replaces its previous timer); starts the daemon if needed |
| `list [--json]` | pane, status, due-in, wall-clock due time, daemon state |
| `cancel <pane_id>` / `cancel --all` | drop timers (a pin that already fired is untouched) |
| `stop` | SIGTERM the running daemon via its recorded pid |
| `daemon` | run the daemon (single instance; a second copy exits 0) |
| `prompt` / `open-prompt` | the popup UI / the action that opens it |

`<delay>` is `90s`, `10m`, `1h30m` (combinable) or a bare number of minutes;
zero, negative and garbage are rejected (maximum 30 days). `<status>` is
`blocked`, `done`, `idle` or `working` (everything `pane.set_status` accepts)
plus the alias `red` = `blocked`. The pane must exist when you arm it.

```sh
python3 timer.py add "$HERDR_PANE_ID" 10m red
python3 timer.py list
python3 timer.py cancel "$HERDR_PANE_ID"
```

The socket is read only from `HERDR_SOCKET_PATH`; without it every command that
needs bora fails with a clear message, so a manual run can never hit a socket
you did not name.

## State and logs

State lives in `HERDR_PLUGIN_STATE_DIR` (bora sets it for actions, hooks and
popups). A CLI run by hand from a shell has no such variable, so it uses the
path bora itself would use: `$XDG_STATE_HOME/<ns>/plugins/ary.pane-timer`, or
`~/.local/state/<ns>/plugins/ary.pane-timer`, with `<ns>` = `HERDR_NAMESPACE` or
`bora`. Files: `timers.json` (atomically replaced), `timers.lock` (mutation lock),
`daemon.lock` (single-instance flock, holds the daemon pid), `daemon.log`
(output of a daemon that `add` or the popup started; truncated when it passes
512 KiB at the next spawn).

A daemon started by the `[[startup]]` hook logs timestamped lines to stdout,
which bora records in the plugin command log once the daemon exits
(`bora plugin log list --plugin ary.pane-timer`). If the socket stays
unreachable for `PANE_TIMER_SOCKET_GRACE_SECONDS` (default 60) the daemon exits
0; the next server start or `add` respawns it. Deadlines use the wall clock, so
a laptop sleep does not postpone them, and a timer that is already overdue when
the daemon starts or wakes fires immediately.

## Limits (each item was observed in a scratch server unless marked otherwise)

- **A fired pin does not survive a server restart or a live handoff**: the pane
  is back to its automatic status afterwards. The plugin does not reapply it.
- **Unfired timers survive a server restart.** Workspaces and pane ids are
  restored (same ids), the timer stays in `timers.json`, and it fires at its
  deadline. Which process fires it depends on the daemon:
  - if you restart before the daemon's 60 s socket grace runs out, the old daemon
    reconnects to the same socket path and fires it; the new server's startup
    daemon logs "another daemon already holds ..." and exits 0;
  - if the deadline passes while both the server and the daemon are down, the
    startup daemon of the next server start fires it right away.
- **Live handoff keeps the old daemon** (it holds the lock; the new server's
  startup daemon exits 0). Pending timers still fire, but the daemon keeps
  running old code until you `timer.py stop` it.
- One timer per pane; arming again replaces it.
- If the pane is closed before the deadline the timer is dropped (logged).
- After a server stop, a daemon lingers up to `PANE_TIMER_SOCKET_GRACE_SECONDS`
  (default 60) before it exits.
- The popup only makes sense in an attached TUI client. The `set` action also
  opens it on a headless server, but nobody can type into it there. While a popup
  is open, a second `set` reports `ui_busy`.

## Tests

```sh
python3 -m unittest discover -s tests -v
/usr/bin/python3 -m unittest discover -s tests -v   # macOS system Python 3.9
```
