---
name: herdr
description: "Control Bora (a Herdr fork), a terminal multiplexer for coding agents. Use only when the user explicitly mentions Bora or Herdr or asks to use it to inspect or control panes, tabs, workspaces, commands, channels, or another agent. Do not use merely because a task could benefit from a background terminal, delegation, or parallel work. Requires HERDR_ENV=1."
---

# Bora

Bora organizes terminals into workspaces, tabs, and panes, recognizes coding agents running inside panes, and exposes the current session through the `bora` CLI. It is a fork of Herdr: the binary is `bora`, the environment variables keep their `HERDR_` names, and a `herdr` command only exists if the user symlinked it.

Before issuing any control command, verify that this agent is running inside a Bora-managed pane:

```bash
test "${HERDR_ENV:-}" = 1
```

If the check fails, say that you are not running inside Bora and stop. Do not inspect or control the focused Bora session from outside Bora.

When the check passes, the `bora` binary in `PATH` talks to the current session. Use it to inspect neighboring work, create terminal layout, start agents and commands, read output, message other agents, and wait for state changes.

## Learn the current CLI

The installed binary is the authority for command syntax. Start with:

```bash
bora --help
```

Then print the relevant command group by running the group without a subcommand:

```bash
bora agent
bora pane
bora workspace
bora tab
bora worktree
bora terminal
bora notification
bora channel
bora integration
bora session
bora machine
bora mcp
bora plugin
bora events --help
```

Do not run bare `bora` for discovery; it launches or attaches the TUI. Bare `bora events` does not print help either: it streams events until interrupted. Do not probe a mutating nested command by omitting arguments. Commands such as `bora workspace create` are valid with defaults and will execute. `bora agent --new` and `bora workspace set-group` print their usage line and exit 2 when run without arguments.

Most control commands return JSON. Read identifiers and state from those responses instead of predicting them.

## Understand layout, panes, and agents

Choose the primitive that matches the job:

- Workspace, tab, and pane topology organize terminal locations.
- Pane commands control raw terminals, shells, tests, servers, input, and output.
- Agent commands control the recognized coding agent currently occupying a pane.

A pane exists whether or not it contains an agent. `agent start` requires an existing available shell pane and never creates, splits, or moves layout. Use pane commands for ordinary processes. Use agent commands when Bora must validate agent identity or interpret `idle`, `working`, `blocked`, `done`, and `unknown` lifecycle states.

Agent commands accept either a unique live agent name or the pane ID currently hosting that agent. They do not accept terminal IDs or bare agent-kind labels. Names must match `[a-z][a-z0-9_-]{0,31}` and be unique among live agents. A name follows the current pane occupant and is cleared when that agent exits, is released, or is replaced.

`idle` and `done` both mean the agent is ready for input. The CLI/API uses the server's seen state to distinguish them; explicit focus commands mark the target seen, while reads do not. Each TUI client tracks viewed completions independently, so its Done badge can differ from the CLI or another client's badge. `blocked` means Bora recognized an approval or question UI. `unknown` means an agent is present but Bora cannot classify it confidently; it does not prove completion.

## Use IDs and caller context

Public IDs are opaque stable handles:

- workspace: `w1`
- tab: `w1:t1`
- pane: `w1:p1`

The number part is a base-32 token, not a decimal counter: `wA8`, `wB4:t1`, and `wB4:p2` are ordinary IDs. Compare IDs as strings.

Closed tab and pane IDs are not reused. A pane moved into another workspace receives a new workspace-qualified pane ID. After `pane move`, continue with `.result.move_result.pane.pane_id` or the live agent name. The old value is reported as `.result.move_result.previous_pane_id`; only the moved process's inherited caller context keeps resolving that old ID, so do not use it as a general agent target.

A user may hand you a pane reference copied from the TUI's pane context menu ("Copy reference"). It has the form `<workspace label> <pane_id>`, for example `bora wB4:p2`. The trailing `w…:p…` token is the pane ID to pass to `agent prompt`, `agent read`, or any `--pane` flag; the label is only there so a human can tell workspaces apart.

Bora injects the caller's context into each managed pane:

```bash
printf '%s\n' "$HERDR_WORKSPACE_ID" "$HERDR_TAB_ID" "$HERDR_PANE_ID"
```

Prefer `--current` when a pane command should target the calling pane. Omitting a target may use the UI-focused pane, which can belong to the user or another client.

Discover live state with:

```bash
bora workspace list
bora tab list --workspace "$HERDR_WORKSPACE_ID"
bora pane current --current
bora pane list --workspace "$HERDR_WORKSPACE_ID"
bora agent list
```

Creation responses expose the IDs to use next. `workspace create` returns `.result.workspace`, `.result.tab`, and `.result.root_pane`. `tab create` returns `.result.tab` and `.result.root_pane`. `pane split` returns the new pane as `.result.pane`.

IDs and live agent names are scoped to one server. Two saved SSH machines can both have `w1:p1` or an agent named `reviewer`. Selecting a machine in the TUI does not retarget commands running in your pane: they still use the inherited session and socket context. Run remote control commands on the intended host with its explicit session, and rediscover IDs there.

`bora machine list` lists saved connection profiles, not a cross-machine pane inventory; add `--json` for scripts. Only add, remove, enable, or disable profiles when the user asks. Removing a profile disconnects the client but does not stop remote sessions. Adding a machine uses the remote default session unless `--remote-session` is explicitly supplied. Setup asks before stopping an incompatible server and defaults to No; do not approve replacement without the user's consent. Experimental handoff is not part of `machine add`.

## Group workspaces into folders

The sidebar's Folders view groups workspaces by a visual group name. Set, nest, or clear it from the CLI:

```bash
bora workspace set-group "$HERDR_WORKSPACE_ID" clients
bora workspace set-group "$HERDR_WORKSPACE_ID" clients/acme
bora workspace set-group "$HERDR_WORKSPACE_ID"
```

A `/` in the name nests folders (`clients/acme` sits inside `clients`). Omitting the name clears the group. The group is display-only: it never changes IDs, worktrees, or which panes an agent can reach. Only regroup workspaces when the user asks.

## Start and coordinate an agent

Default to a sibling pane in the current tab and the current working directory. Do not create a workspace, tab, worktree, or different cwd unless the user explicitly requests that topology or location.

Honor a direction requested by the user. Otherwise inspect the caller pane:

```bash
bora pane layout --pane "$HERDR_PANE_ID"
```

Split a wide pane to the right and a narrow or tall pane down. Avoid repeated same-direction splits that create unusably narrow columns or short rows. Keep the user's focus in the calling pane and explicitly preserve the caller's working directory:

```bash
bora pane split --current --direction right --cwd "$PWD" --no-focus
```

Replace `right` with `down` when appropriate. Read the new pane ID from `.result.pane.pane_id`.

An available shell pane must be at its interactive prompt, with the shell itself in the foreground and no foreground command, editor, or agent running. Start a supported agent in that pane with a useful unique name:

```bash
bora agent start reviewer --kind codex --pane <returned-pane-id>
```

Use the kind requested by the user. Run `bora agent start --help` to inspect the installed kind list and options. Pass native agent arguments only after `--`:

```bash
bora agent start reviewer --kind codex --pane <returned-pane-id> -- <agent-args...>
```

A successful `agent start` returns only after Bora detects the expected agent in the same pane and considers it ready for interactive input. If the agent is blocked during startup, the command returns `agent_not_ready` immediately but keeps the name available for `agent read` and `agent send-keys`. Wait until the agent becomes idle before prompting it. Startup defaults to a 30-second timeout.

Submit work through the agent surface:

```bash
bora agent prompt reviewer "Review the current diff and report only actionable findings." --wait --timeout 120000
```

`agent prompt` honors the pane's live bracketed-paste mode and sends text followed by encoded Enter as one ordered submission. It reports successful submission only after both have been written; that alone does not prove the agent started a turn. The submit delay grows with prompt size for Codex on Windows. It rejects an agent already waiting at an approval or question dialog with `agent_blocked` before sending any input. Inspect the blocked UI and ask the user before answering it. For normal agent work, `--wait` is enough: it waits for the first settled `idle`, `done`, or `blocked` state. Do not repeat those defaults with `--until`.

By default the prompt is injected immediately, even while the agent is mid-turn (steering). Add `--when-idle` to hold submission server-side until the target leaves `working`; pair it with `--timeout` so the hold is bounded. The CLI attributes the prompt to your pane from `HERDR_PANE_ID`; `--from <pane>` overrides that and `--no-from` suppresses it.

With `--wait`, a prompt sent from a non-working state must produce observed `working` or `blocked` activity. After submission, Bora waits up to five seconds for that activity; unrelated `idle`, `done`, or session changes do not satisfy this gate. It returns `agent_prompt_stalled` if no activity is observed, or `timeout` if the caller's timeout expires first. The caller timeout includes submission time. Without a timeout, the settled-state wait is indefinite after activity is observed. This wait tracks lifecycle state, not an individual turn; if the agent is already working, completion of the active turn may satisfy it.

Use `--until` only for a state-specific workflow, such as waiting for an already-running agent to request input:

```bash
bora agent wait reviewer --until blocked --timeout 120000
```

Without `--until`, standalone `agent wait` uses the same settled-state defaults as `agent prompt --wait`.

Use logical keys for interactive agent UI controls:

```bash
bora agent send-keys reviewer esc
bora agent send-keys reviewer ctrl+c
```

Bora validates all keys before writing any bytes. Read the result through the resolved agent:

```bash
bora agent get reviewer
bora agent read reviewer --source recent-unwrapped --lines 120
```

If a wait fails or returns `blocked`, inspect `agent get` and `agent read` before deciding what input to send. A timeout or stalled response does not prove the prompt was never delivered; do not blindly submit it again. Use the pane surface only when raw terminal control is intentional.

### Dispatch an agent in one command

When the user wants a fresh agent working on the current directory and does not care about pane placement, two fork-only forms compose `workspace create` (on `$PWD`, without stealing focus), `agent start`, and `agent prompt` into one JSON result:

```bash
bora agent --new "review the plan and list the risks" [--kind KIND] [--name NAME]
bora agent <name> prompt "first task" [--kind KIND]
```

`--new` always creates: two runs give two agents, and a derived name that is already taken gets a `-2`, `-3` suffix. `agent <name> prompt` is get-or-create: the name is the idempotency key, so an existing agent with that name only receives the prompt (`.result.created` is `false`), and a missing one is created with exactly that name. The kind resolves `--kind`, then `[agents] default` in config.toml, then `omp`. The prompt is injected without `--wait`; follow up with `bora agent wait <name>` when you need the result. Because both forms create a whole workspace, prefer the sibling-pane flow above unless the user asked for a separate workspace.

## Run an ordinary command in another pane

Create a sibling pane with the same geometry rule, preserve the caller's working directory, and keep user focus unchanged:

```bash
bora pane split --current --direction right --cwd "$PWD" --no-focus
```

Read the new pane ID from `.result.pane.pane_id`, then run and inspect the command:

```bash
bora pane run <returned-pane-id> "just test"
bora pane wait-output <returned-pane-id> --match "test result" --timeout 120000
bora pane read <returned-pane-id> --source recent-unwrapped --lines 120
```

`pane run` atomically sends command text and Enter. `pane wait-output` searches the selected snapshot immediately, so output that already exists can match. Use `--match <text>` for a literal substring or `--regex <pattern>` for a Rust regular expression. Omitting `--timeout` allows an indefinite wait.

Use the read source that matches the task:

- `visible`: the currently rendered viewport.
- `recent`: recent rendered output, including soft wraps.
- `recent-unwrapped`: recent output with soft wraps joined; prefer it for logs and transcripts.
- `detection`: the plain-text bottom-buffer snapshot used for agent detection.

Use `--format ansi` when colors and terminal styling are evidence. Otherwise use text.

`--lines` asks Bora for more rows from the pane's available screen and host scrollback. If increasing it does not reveal more of a completed response, the pane is probably running the agent on the terminal's alternate screen. Rows that leave the alternate screen do not enter Bora's host scrollback, so a larger line count cannot recover them.

After that failed read, ask the agent to write its complete response as Markdown in a temporary directory and reply only with the file path, then read the file directly. Use this only as a fallback; do not request file output in the initial prompt.

## Watch session events

`bora events` streams session events as one JSON object per line on stdout until interrupted:

```bash
bora events --limit 1
bora events --subscribe pane.created --subscribe pane.closed
bora events --subscribe pane.agent_status_changed --pane <pane-id> --limit 1
```

Without `--subscribe` it covers every event that needs no pane (`workspace.*`, `tab.*`, `pane.created`/`closed`/`exited`, `channel.message`, `layout.updated`). The pane-scoped subscriptions (`pane.agent_status_changed`, `pane.output_matched`, `pane.scroll_changed`) require `--pane`, and `--pane` without `--subscribe` is a usage error. `--limit N` exits after N events; without it the stream is indefinite, so pass `--limit` or run it in a pane you can interrupt. Streams start live and never replay history. `--session <name>` targets a named session.

## Channels

A channel is a workspace named `#name` that groups agent panes for broadcast messaging. Use it to coordinate with other agents instead of `agent prompt`-ing each one individually.

```bash
bora channel send eng "done with the refactor" --current
bora channel send eng "@reviewer please check this"
bora channel send eng "please check this" --to reviewer
bora channel send eng "checkpoint reached, read when free" --when-idle
bora channel send eng "yes, ship it" --reply-to 42
bora channel note eng "lint is green on main"
bora channel ask eng reviewer "is the migration reversible?" --timeout 120000
bora channel members eng --json
bora channel tail eng --after 42
```

- `channel send <name> "<text>" --current` replies in the channel; the sending pane resolves from `$HERDR_PANE_ID` automatically.
- A leading `@nick ` token in the text addresses one member pane. An unknown or ambiguous nick fails the send with `channel_nick_unknown` or `channel_nick_ambiguous`: nothing is delivered or recorded. Pick a real nick and send again. Broadcast happens only when the text has no leading `@`. `--to NICK` is the same rule as a flag.
- A nick is the member's `pane_id`, that ID with the colon dropped (`wB4p2`, always unique), or its `name` from `channel members --json`. Two panes of the same agent kind share the name form; address those by pane ID. Escape a literal `@` or `#` as `\@` and `\#`.
- Delivery is immediate by default, even into a member that is mid-turn. `--when-idle` holds delivery for each working member until it is free; that member's receipt says `deferred`, which means queued, not failed. Do not resend it.
- `channel note` appends to the transcript without injecting into any pane. `channel ask <name> <nick> "<text>"` blocks until the member replies with `channel send … --reply-to <seq>` or the timeout (default 300000 ms) expires; a timeout is a clean `answered: false`, not an error.
- `channel tail <name> --after SEQ` catches up on messages missed since the last seen `seq`, reporting a `#gap:` notice on stderr if log rotation dropped anything in between. `--follow` keeps watching; `--json` emits structured lines.

Bora injects a one-time protocol block into a pane's own terminal the first time it joins a channel or receives a channel message, teaching the same verbs from inside the agent's session. This section is discovery; that injected block is the binding contract for a pane already on a channel.

## Safety and coordination rules

- Use `--no-focus` for background work unless the user asked to switch context.
- Use `--current`, an explicit pane ID, or a unique agent name. Do not rely on another client's focused pane.
- Parse IDs from JSON responses. Do not derive them from sidebar order or examples.
- Do not close workspaces, tabs, panes, or sessions you did not create unless the user explicitly asked. `workspace close --group` closes the primary workspace and its linked worktree workspaces; never add it merely to bypass `workspace_group_close_required`.
- Use `--trust-repository` only after the user has verified the repository. It grants per-request Git trust; it is not a routine retry for a failed worktree command.
- Client and server versions can differ after an update. Check `bora status` before relying on new server features. A missing method is not permission to stop or upgrade a server.
- Never run `bora server stop` from an active session unless the user explicitly intends to stop the server and its pane processes.
- Never kill the main Bora process. Use named test sessions for experiments that need an isolated server.
- CLI server errors are JSON on stderr with exit status 1. CLI syntax errors exit with status 2.
