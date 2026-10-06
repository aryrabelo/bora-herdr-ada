# ary.windhover-push

Push notifications to the [Windhover](https://github.com/aryrabelo/windhover) iPhone app
when an agent in bora needs you, finishes, or asks you a question. The text is sealed on
this host with the phone's own key (AES-256-GCM) before it leaves; the relay and Apple
only carry ciphertext. No core changes: a plain plugin with two event hooks.

## Install

```sh
bora plugin install aryrabelo/bora-herdr-ada/examples/bora/plugins/windhover-push
# or, from a checkout:
bora plugin link examples/bora/plugins/windhover-push
```

It needs `bun`, or `node` 20 or newer. bora starts hooks with the server's environment,
whose `PATH` can be minimal, so `run.sh` also looks in the usual install locations
(`~/.bun/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`); set
`WINDHOVER_PUSH_RUNTIME=/path/to/bun-or-node` in the server's environment to pin one.
There are no npm dependencies and no build step.

Then turn on "Notify me from this host" for the connection in Windhover. The app writes
one device file per phone over SSH; until one exists the plugin sends nothing.

## What notifies

| bora event | push | level |
| --- | --- | --- |
| pane `blocked`, still `blocked` in `pane.get` 3 s later | `<workspace>: <agent> needs you` | `needsYou` (Time Sensitive) |
| pane `done`, or `idle` right after `working` | `<workspace>: <agent> finished`, at most once per pane per minute | `finished` |
| pane `working`, `unknown`, `idle` after anything else | nothing | |
| `channel.message` with `kind == "ask"` and `to_human == true` | `<from_name> asks`, body = the first 120 characters of the question | `needsYou` |
| two or more such asks within 3 s | one `<N> questions waiting`, body = the askers' names | `needsYou` |
| any other `channel.message` | nothing | |

The body of a pane push is the pane title bora reports in the event (empty when it has
none). `<agent>` is the event's `display_agent`, else its `agent`, else the word `agent`; `<workspace>`
is the workspace label from `workspace.get`, else its id.

The plaintext is `{title, body, connectionId, ask?}`: `connectionId` comes from the device
file, so tapping opens that connection; `ask: {channel, seq}` is set only on a single-ask
push and opens the app's "Needs you" screen on that question. A coalesced push has no
`ask`, since it stands for several.

Relay ids (both 64 lowercase hex chars):

| push | `collapseId` | `threadId` |
| --- | --- | --- |
| pane | `sha256(pane_id)` | `sha256(workspace_id)` |
| ask | `sha256("ask:" + channel + ":" + seq)` | `sha256("channel:" + channel)` |
| coalesced asks | none | `sha256("channel:" + channel)` when they share one channel, else none |

`kind` and `to_human` arrive in bora's `channel.message` event with the channel-asks
change; on a server without them, every channel message is ignored and pane pushes still
work.

## Device files

`~/.config/windhover/push/<device-id>.json` (`WINDHOVER_PUSH_DIR` overrides the directory),
written by the app with `umask 077`:

```json
{"version":1,"deviceToken":"<hex>","environment":"production","key":"<base64 of 32 bytes>",
 "relay":"https://windhover-push-relay.aryrabelo.workers.dev","label":"iPhone","connectionId":"<UUID>"}
```

Every push goes to every valid file, sealed with that file's `key`, as
`POST <relay>/v1/push {deviceToken, environment, level, e, collapseId?, threadId?}`. When
the relay answers `{"source":"apns","status":410}` or
`{"source":"apns","status":400,"reason":"BadDeviceToken"}` the plugin deletes that file:
the phone uninstalled the app or the token died, and the app writes a new file when it
gets a new token. The file is deleted only if it still holds the token that was sent, so a
registration rewritten while the push was in flight survives. Relay-side errors
(`"source":"relay"`, a timeout, an unreachable relay) never delete anything. A file that
does not parse is skipped and left alone.

## Why hooks, not a startup daemon

Each `[[events]]` entry runs `run.sh hook` once per event; the 3 s recheck, the cooldown
and the ask window are kept in files under `HERDR_PLUGIN_STATE_DIR`, changed only while
holding `state.lock`. A `[[startup]]` daemon on `events.subscribe` would keep that state in
memory, but:

- `[[startup]]` does not run on `bora plugin install` or `link`, only at the next server
  start or live handoff, so a daemon would need its own bootstrap and a single-instance
  lock (a handoff starts a second copy), and Node has no `flock`.
- A daemon holds one of the server's 32 plugin-command slots for its whole life and dies
  with its socket on every restart; nothing supervises startup commands.

What hooks cost instead: two kinds of hook hold their slot for 3 s more (a `blocked` hook
before its `pane.get` recheck, and the first ask of a window before it flushes the
batch), and both sleep only when a device file exists. A hook that sends holds its slot
until every relay answers, at most 10 s (the request timeout) when a relay is slow or
down; a hook that sends nothing exits at once.

The recheck is best effort. Every pane event bumps a per-pane generation number; a
`blocked` hook sends only when, after 3 s, bora still reports the pane `blocked` and no
newer event for the pane arrived, so a flap (`blocked → working → blocked`) normally sends
once, from the newest hook. A change that lands after that check, during the workspace
lookup and the relay request, can still let a stale "needs you" through. The cooldown and
the ask window are decided inside the lock, so concurrent hooks never both send them.

## State and logs

`HERDR_PLUGIN_STATE_DIR` (bora sets it for hooks) holds `panes.json` (last status,
generation and last "finished" time per pane; entries idle for 7 days are dropped),
`asks.json` (the open ask window) and the transient `state.lock`. Each hook prints one line
per device to stdout (`bora plugin log` shows it): the device file, its label and the relay
answer. Notification text and device tokens are never logged.

## Limits

- Done after the 60 s cooldown only: two "finished" pushes for the same pane inside a
  minute collapse into the first. The cooldown starts when a push is attempted (at least
  one device file exists), even if the relay then fails; there is no retry.
- Asks are coalesced across devices as a whole, because every device receives every push.
- With no client attached, bora reports a finished pane in the active tab as `idle`, not
  `done`; that is why `idle` right after `working` counts as finished.

## Tests

```sh
node --test tests/*.test.mjs   # Node >= 20
bun test tests/
```

`tests/fixtures/push-envelope-v1.json` is a verbatim copy of Windhover's
`Tests/Fixtures/push-envelope-v1.json`; `envelope.mjs` must open every valid vector,
reproduce every envelope byte for byte, and reject every invalid one. Do not regenerate it
here: change the vectors in Windhover and copy them over.
