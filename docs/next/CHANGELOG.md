# Changelog

Bora is a fork of [herdr](https://github.com/ogulcancelik/herdr). This changelog records bora's own changes; changes pulled from upstream herdr during a sync are grouped under a "Synced from herdr" heading.

## Unreleased

### Added

- `bora pane list` carries a `foreground_process` field: the name of the process currently in the pane's foreground process group (e.g. `omp`), so callers no longer need a second `pane process-info` round trip just to see what is running in a pane. The field is filled only in the `pane list` response, never in `pane.created`/`pane.updated` events, keeping a process-table read out of the event fan-out path. `pane process-info` now also resolves the pane's controlling terminal (`tty`, e.g. `/dev/ttys003`) from the shell's process info instead of always returning `null`.
- The sidebar answers "how many panes are waiting" at a glance: the workspace list's top row now shows an aggregate `N waiting` counter — attention-yellow while unseen panes have finished, red the moment one of them is blocked and waiting for you. Behind it, a pane in any background workspace that produces no output for `ui.idle_attention_seconds` (new `[ui]` key, default 300, `0` disables) is promoted into the same unseen-attention state the finished/blocked dots already use — so a plain shell command that exits, or an agent that goes quiet, flags its row even when no agent-state transition fires. Panes start unpromotable until their first output (a restored session never mass-flags on startup), the active workspace and channel workspaces are exempt, and focusing a workspace clears its promoted panes.
- New `folders` sidebar view (`ui.view_mode = "folders"`, or cycle it with the view-mode toggle/`prefix+shift+v`): a flat list like Flat, but it honors your own `visual_group` folders and nothing else — no repo auto-grouping, no branch brackets, no `@wNpN` pane badge. Each workspace is ONE line — its name followed by one clickable ○/spinner dot per pane on the same row — instead of a single indicator or a two-line block. Assign a workspace to a folder with the workspace context menu, `bora workspace set-group`, or by dragging its row onto a folder header. A separate `ui.hide_pane_badges` toggle drops the synthetic `@wNpN` badge (a registered `bora agent rename` name is kept) in the other views.
- Project view sidebar redesign: group headers lose the hexagon and gain an underline; one section row per workspace (collapse chevron, bright uppercase name, dim branch) with a right-aligned git/PR state cluster (ahead/behind, uncommitted/staged markers, PR number + checks rollup, unknown never green); worktree workspaces render as full workspace sections marked ⌗; every pane gets its own ○ row; a blank row now separates workspace blocks (`ui.sidebar.project.row_gap`, default 1); state icons ship in plain unicode by default with an opt-in Nerd Font set (`ui.sidebar.project.glyph_style = "nerd_font"`).
- Colliding channel names are resolved at join time instead of leaving a room where nobody is addressable. Two members answering to the same name — most often two panes detected as the same tool — now take ordinals in join order: `@rev` becomes `@rev-1` and `@rev-2`, `channel members` lists the renamed form, and the channel gets a line naming the pane that joined and the name it answers to now. The ordinal is derived from the join roster rather than recorded, so the same join order always mints the same names across restarts and there is no second source of truth for a `channel leave` or a workspace rename to leave stale. Addressing the bare colliding name is still refused as ambiguous rather than amplified to every holder: the suffixed name is the one that reaches exactly one pane.
- Right-click a folder header in the Folders (or Repo) sidebar view to rename the group. "Rename group…" opens an input prefilled with the current name; confirming moves every workspace in that folder to the new name and carries the folder's collapsed/hidden state across. Renaming onto an existing folder's name merges the two.

### Fixed

- In the Folders sidebar view, the row carrying the active `▎` marker now fills with the theme's active-row background, so the workspace you are in stands out from the folder's other rows instead of differing by a two-pixel bar. The fill is the same `active_row_bg` the focused agent rows use, honours `[theme.custom]` overrides, and is scoped to Folders — Flat, Repo and Project rows keep the bar-only statement.
- Folders view answered the mouse like Repo instead of like Flat. Because `groups_workspaces()` is true in Folders, three Repo-era drag rules leaked into it: a linked worktree row — which is nearly every row — refused to open a drag at all, so a press-and-move resolved as a click on release and the workspace switched instead; a drop that did land moved the whole repo sibling block along with the grabbed row; and the drop slots between two same-repo siblings were suppressed, so the row landed on the wrong side of the pair. Folders now drags exactly like Flat: one row per grab, a slot in every gap, and dropping onto a folder header assigns the folder as documented.
- Dragging a workspace into a folder works from anywhere inside the folder, not just its one-line header. The reparent-on-drop only matched the thin folder-header row, so dropping onto the folder's actual member rows just reordered the list and the row snapped back under its own header — leaving the right-click "move to group" menu as the only reliable way in. A drop onto a folder header or any of its member rows now moves the workspace into that folder; a drop in the ungrouped area still reorders. (A drop on a grouped member means "join", so rows can no longer be reordered within a folder by dragging; removing a workspace from a folder stays a right-click action.)
- A channel mention that does not resolve no longer broadcasts to everyone. `@nick` in the body of `bora channel send` used to degrade silently — an unknown or ambiguous nick was logged at debug level and the text went out to every agent member pane, each one reached by typing it into that pane's live session. One intended recipient became N deliveries, in N sessions, and the sender was told the send succeeded. It also hid behind a collision it caused: a nick resolved against the detected agent kind, so two panes both detected as `omp` made `@omp` ambiguous, and ambiguous meant broadcast. Now an unresolved mention fails the send with `channel_nick_unknown`/`channel_nick_ambiguous` — nothing appended, nothing delivered — exactly like `--to` always did; broadcast requires text with no leading `@`. Prose that genuinely starts with `@` escapes as `\@`.
- Channel identity is the workspace label. The name a member is listed and addressed by now starts at the workspace's own label — the name the sidebar shows — before the agent's self-reported name, its assigned name, and its detected kind. Attribution and resolution had drifted apart in the one way that matters: the sender line was built from the workspace label while matching never looked at it, so a member attributed as `ceo-bora` answered to nothing of that name, and the only nick that did resolve was the kind it shared with its neighbour. A `#`-prefixed label is refused as an identity, since that names a channel rather than an agent; panes native to a channel workspace keep their own names, and such a sender is no longer attributed as the channel itself.
- Server mode froze every spinner. The headless tick never advanced the animation clock (`spinner_tick`) nor armed its timer — both lived only in the standalone app's tick — so in server mode, the mode most operators actually run, the Project view's working dot sat on one frame and idle age labels never counted. The decision now lives in one shared `App::tick_animation` helper (advance when due, then re-arm) called by both tick paths, the same drift-proofing the projects.yml poll already went through.

- Two projects that declare the same directory no longer collapse into one group in the Project view. A workspace's project was derived purely from its path — repo identity plus subdir — so a directory declared by more than one project had no way to say which one owned a given workspace, and the first project in slug-alphabetical order silently claimed all of them. With `worktrees: all` on a member, which is the common case, that meant every worktree of a repo landed in the same group no matter which project you created it under. A workspace now remembers the project it was created under and that binding wins over the path derivation; a binding pointing at a project that no longer exists falls back to the old behaviour instead of orphaning the workspace. The derivation itself also got a tiebreak, so even an unbound workspace on a contested directory now goes to the project whose member is more specific (`worktrees: this` before `all`, deeper subdir before shallower) rather than to whichever slug sorts first. `bora workspace set-project` rebinds a workspace that is already grouped wrong, and the binding survives a restart — it is written into the session snapshot and read back on restore.
- Project view rows answer the mouse again. A workspace row in the Project view emitted no workspace hit area, so everything workspace-scoped silently missed it: clicking a row painted no selection or active highlight, right-click offered only the project-membership items instead of the full workspace menu, and drag-to-reorder could not even start because no press was ever recorded. One missing hit area caused all three. Rows now emit it, so selection and active backgrounds paint, right-click opens the full workspace menu with the membership items still spliced in, and a row can be dragged to reorder — including a linked worktree, which in this view is its own top-level row rather than an indented child.
- Dragging a workspace onto another project's header moves it into that project.
- The sidebar's view-mode toggle is back. Cycling Flat/Repo/Project by mouse was removed together with the `spaces` title it shared a row with; only the title was meant to go, which left the keybind and the settings dialog as the only ways to change view. The current view's name is clickable again, right-aligned on the workspace list's first row; the title stays gone.
- Renaming a workspace to an empty string no longer leaves it permanently nameless. `bora workspace rename <id> ""` stored the empty label, and a custom label wins over the automatic one, so the row rendered blank in every view and in the tab bar. A blank rename now clears the label and the automatic name takes over.
- Two workspaces with the same name inside one project group are told apart, by branch where the branch differs and by parent directory otherwise. Previously a differing branch exempted them from disambiguation entirely, which left two visually identical rows on screen.
- On the 16-colour `terminal` theme, `mauve` was the same grey as ordinary muted text, so every mauve accent — the Project view header name, the worktree marker, the merged-PR chip — disappeared into the surrounding text; it now maps to the unclaimed ANSI purple slot. `surface0` was likewise identical to the sidebar background, so rows asking for a slightly lighter fill got none.
- Project view: a workspace's panes share ONE row instead of one row each. The row carries the workspace's own unique name followed by a state dot per pane, and each dot is its own click target that focuses that pane. This drops the per-pane id, which said little for a whole row, and removes the `╰` connector that used to hang off the last pane row only — an asymmetric mark that read as a stray glyph rather than as structure.
- Project view: within one group, a second worktree of a repo whose name is already on screen renders a `───────` rule in place of the name instead of repeating it. Two worktrees of one repo previously printed the full repo name twice, spending the row's most valuable column on the one thing that had not changed; the workspace's own name distinguishes them on the row below. The parenthesised parent-directory suffix that used to disambiguate them disappears from that line as a consequence.
- Project view: clicking a workspace row's name selects the workspace instead of collapsing it. The whole row was a collapse target, so a click could never mean "select this", and pane rows were not clickable at all. The chevron and its adjoining space collapse; everything else on the row selects and focuses.

### Changed

- Catppuccin theme: the Navigate-mode cursor row in the sidebar uses a brighter background (surface2 instead of surface0), which at one step above the sidebar background was nearly indistinguishable from an unselected row. Any `[theme.custom_colors] selection_bg` override still wins.
- Project view: the group header carries a slightly lighter background and shows its caret only when collapsed, and its name is italic rather than bold — the background supplies the emphasis, and italic leaves the header on a font channel of its own so a display face can be aimed at it without repainting every branch label. Repo and branch on a workspace row are dimmer, letting the name and the state cluster lead.
- Project view: a pane dot's hue follows the owner's color ruling — red = the pane stopped and is waiting on you (blocked and unread, joining the read falha ◆; the diamond now only says the falha was already read), yellow = finished, come read it, gray = quiet — and the workspace's own name takes its panes' most urgent dot colour instead of staying grey, so a line that stopped or finished lights up whole rather than as a lone bullet. The working dot animates the shared `sand` fill-and-drain set at the working cadence, replacing a single-cell braille spinner whose motion did not read as motion. The project header band keeps its padding row only above its text; the breathing room below it is plain background again, because the padded band read as oversized and sat glued to its first block.
- Chat view Messages column groups consecutive messages by sender instead of repeating a right-aligned nick on every line. Each run of adjacent same-sender messages now opens with one `<name> · DD-Mon` header, and messages render under it in a 9-column gutter (2-col indent + `HH:MM` + 2-col gap) — replacing the old fixed 21-column sender column, which spent a fifth of the timeline's width restating a name that rarely changed line to line and made a burst from one sender read as N disconnected rows instead of one block. A message's destination is now a short inline token before its text rather than a glyph or a full-line highlight band: `→você` renders as a bg-accent badge scoped to that token only (never the whole line, which used to make an addressed-to-human message's entire wrapped block flash the accent colour), and a `to_pane` destination resolves through the pane's real addressable name (cached in `ChatViewState.to_names` at data-refresh time, via the existing `App::pane_display_name` identity chain) instead of the old bare `›` glyph.
- Channel messages addressed to the human seat (`to_human`) now deliver passively, per the owner's 2026-08-29 decision (ceo-bora#33): arrival never auto-opens the chat view over whatever pane is on screen and never switches the active workspace — it only refreshes the channel's own sidebar row (a `chat_unread` badge carrying a dim one-line `<sender>: <text>` preview, drawn through the same `Workspace::metadata_tokens` path `bora workspace report-metadata` already used, so it needs no sidebar config to show) and, while nothing else already shows it, raises the existing NeedsAttention toast. Reading a message stays a deliberate `prefix+i` open, which is also what clears the badge. This replaces the `ui.chat_open_on_mention` auto-open added in 0.23.0 — the config key, its `human_last_input_at` typing-window guard, and the closed `Mode` allowlist it drove are all gone; a config file still setting the key now has it silently ignored rather than getting a behaviour that could hijack an active session.

## [0.45.5] - 2026-08-25

### Added

- `bora agent --new "<prompt>"` turns "I want an agent working in this directory" into a live session in one command: it creates a workspace on the caller's cwd (without stealing focus), starts the configured agent on its root pane, injects the prompt, and prints one JSON (`workspace`, `pane`, `agent`, `prompt_delivered`, `created`) — the one-shot verb external dispatchers need (Paseo command profiles are bare `<command> {{{prompt}}}` templates, which cannot express the old three-command-plus-JSON-surgery choreography). The kind resolves `--kind` over the new `[agents] default` config key (validated like `[agents.commands]`) over a hardcoded `omp` fallback. The positional form `bora agent <name> prompt "<text>"` is get-or-create: the name is the idempotency key — an existing agent just gets the prompt (`created: false`), a missing one is created with exactly that name, and a creation race (`agent_name_taken`) prompts whoever won it, so idempotency falls out of the name-uniqueness invariant the server already enforced. No new server flow: the composition reuses `workspace.create` → `agent.start` → `agent.prompt` client-side.
- A sidebar row now looks like what it is. An agent's row takes its colour from its own status dot and its weight from how much it wants you, so the four states that previously rendered as four identical grey rows distinguished by a single glyph — finished-but-unseen, working, idle, no agent — are now told apart at a glance, and told apart by weight alone on a terminal without colour. The active row is marked by a single-column edge bar instead of having its whole background repainted, because a full-row fill is how a list says "selected" and focus is a lighter statement than selection; the practical gain is that an active blocked row now reads red on its own merit rather than being painted over by the selection colour. Agent rows also sit under their branch instead of one column shallower than it, which is what makes the tree scan as a tree.
- Sidebar bands are an open set now, not a fixed five. A band is a descriptor in a registry — its wire name, glyph, label, counter format, bullet style, the level it may appear at, and its row-pushing function — so adding one costs a registry entry and one function instead of the nine scattered edits it cost before: an enum variant, a fixed-length `ALL` array whose size cascaded into three signatures, and seven separate `match` arms across two files, four of which the compiler would not have caught. The level is now something a band DECLARES rather than something two hand-maintained narrowing functions imposed, which is why the `unreachable!()` arm that existed only because the old shape could not express "this band is worktree-level" is gone. `sections.order:` is unchanged, including its tolerance for a name it does not recognise. Nothing allocates: the registry is a slice whose length is a compile-time constant, ordering still returns a fixed-size array, and level filtering is a borrowing iterator.
- The tab bar tells you which agent is waiting on you. It previously said nothing at all about agent state — a blocked agent one tab away was invisible — so a tab now carries its most urgent pane's state, ranked the same way the sidebar ranks it. The three cases stay distinct because collapsing them is what makes an indicator useless: a bold red diamond means an agent is blocked asking you something right now, a dim amber marker means one finished while you were elsewhere, and a dim spinner means one is still working and wants nothing.
- A project can declare the order of its sidebar bands: `sections.order: [notes, todos, checks, commands]` in `projects.yml`. Unknown names are ignored so a newer bora writing a fifth band name cannot break an older binary's sidebar, a duplicate is honoured once at its first position, and — the part that matters — a partial list does not hide anything: bands you leave out still render, appended after the ones you named. Ordering decides sequence, never visibility, so a band that renders nothing today still renders nothing when you list it first. Absent `order:` keeps today's fixed sequence exactly. Project-level bands (TODOS, NOTES) and worktree-level bands (COMMANDS, CHECKS) still reorder only within their own level; they never interleave.
- Repo commands now read wt's `.wt/settings.toml` `[scripts.run.*]` schema (precedence `.wt/settings.local.toml` > `.wt/settings.toml` > `.conductor/settings.toml`, merged per id and per field) as the single command format; `.bora.toml` `[[commands]]` still parses but logs a deprecation warning. Definitions are cached per repo root with mtime invalidation plus a one-second probe throttle, so repeated loads — including the sidebar's — perform no filesystem stats at all inside the window. A command launched in pane mode tags its pane with the command's label, so the COMMANDS band can count it; shell-mode commands stay fire-and-forget and uncounted.
- The worktree COMMANDS band is live: `n/m` counts selected commands with at least one live tagged pane over the project's selected pane-mode declarations, running rows are marked, and clicking a row launches the command into the worktree's representative workspace with `$BORA_PORT` resolved. The old Programs launcher band (and its "run command…" prompt mode) is deleted — the section replaces it. Command definitions refresh onto each workspace on the runtime tick; the render path never touches the loader.
- CI checks are a provider contract, not a GitHub integration: a provider is a command template that answers `{repo, dir?, branch}` with JSON `[{name, status, conclusion}]`, and the built-in `gh` provider (today's `gh pr view` path, output unchanged) is one implementation. Provider failure renders an explicit error row — never a silently empty band — while not-applicable (no PR, no provider) renders nothing. The CHECKS refresh interval is configurable (mirroring `[github] refresh_interval_secs`, default 30s unchanged).
- Project-scoped todos and scratchpads: two append-only, cursor-replayable stores (channels pattern) with socket verbs `todo.create`/`complete`/`list` and `scratchpad.write`/`append_section`/`find`, `TodoChanged`/`ScratchpadChanged` events (wired for both subscribers and plugin hooks), and MCP tools for all six verbs. A todo carries title, open/done state, blockers, assignee, and origin; blocked todos are excluded from the actionable listing at store, verb, and sidebar level. The sidebar renders TODOS (`n/m` = done/total, one row per actionable todo) and NOTES (one row per scratchpad doc) between the project row and its worktrees, refreshed by the verbs and on project reload — never by render.
- `[agents.commands]` config: per-agent-kind override for the executable `bora agent start` types into the target pane, keyed by canonical agent id (e.g. `omp = "omp-raw"`). Falls back to the built-in canonical executable name for any kind without an override. Useful when an agent's normal command is actually a local shell function or alias (a sandboxing wrapper, for example) and you want `agent start` to bypass it.
- Chat view nicks are coloured per sender, deterministically, so a wall of agent traffic is scannable by colour before you read a name. The accent colour stays reserved for the human seat.
- `bora mcp serve` offers ten more verbs as MCP tools — `agent_start`, `agent_read`, `agent_wait`, `pane_read`, `pane_process_info`, `pane_wait_for_output`, `events_wait`, `events_subscribe`, `plugin_action_list`, `plugin_action_invoke`: none of their params carries a top-level channel `name` or a `from_pane`, so the `$HERDR_PANE_ID` default is unchanged and no new top-level fencing entry was needed (`events_wait` does carry a channel one level down, and is fenced there — see Fixed). An MCP-speaking agent can now spawn and read other agents, wait on agent status, pane output, or events, inspect pane process trees, and list and invoke plugin actions; `agent_prompt` still requires `--allow-prompt`.
- Chat view: long messages no longer eat the timeline. A message is clamped to 8 display lines with an explicit `… +N lines` marker showing the true count of hidden wrapped lines; clicking the message (or its marker) expands it in full, and clicking again collapses it. Expansion is one message at a time — the room stays readable while you read whichever post you actually care about.
- New `project.*` socket verbs — `project.list`/`.create`/`.update`/`.member_add`/`.member_remove` — read and write `~/.config/bora/projects.yml`, the "Composition model" declaration a project groups member directories under. Writes go through an atomic tmp-file-plus-rename so a concurrent reader never observes a half-written file, `create` refuses to overwrite an existing slug, `member_add` is idempotent on a dir already present, and `list`/every mutation response resolves each member's repo identity so a caller never has to redo git discovery itself.
- Chat view: the composer is a framed control now — a bordered panel with a `[ Chat ]` title and a live character counter on its top border, matching the frame every other panel uses. The counter is derived from the draft at render time and never stored; the timeline gives up the two rows the frame costs.
- Chat view: the channel list, timeline, and member list are each a framed panel now — `[ Channels ]`, `[ Messages ]`, `[ Members ]` — replacing the old single-character column separator. Every column's own border eats two columns and two rows from its content, so name/label truncation was recomputed against the smaller inner width: nothing renders hard-clipped without an ellipsis.
- `bora mcp serve` exposes the `project.*` verbs as MCP tools — `project_list`, `project_create`, `project_update`, `project_member_add`, `project_member_remove` — so an orchestrator can assemble its own project at runtime: create it, point it at a channel, add member worktrees. `project_create`/`project_update` carry a `channel` string; audited and deliberately left unfenced: naming a channel in `projects.yml` grants no read of its traffic (every traffic verb keeps its own `--channels` fence on its top-level `name`), and their `name` field is the project display name, which the channel fence must never compare against channel slugs.
- Any plugin can put an entry in a right-click menu, by declaring `contexts` on an `[[actions]]` entry in its manifest. `PluginActionContext` (`global`, `workspace`, `tab`, `pane`, `selection`) was already parseable and already round-tripped over the wire, and was read by exactly nothing; the menus now consult it, so placement is a plugin's own declaration rather than a bora source change. `global` appears in every menu; a context that matches nothing, is absent, or is unrecognised is a silent skip — no greyed-out row, no stray separator, nothing logged — and a disabled plugin contributes nothing. Selection resolves through the same `find_plugin_action` lookup keybinds and the CLI already use, so a menu entry and `bora plugin action invoke` cannot drift. This replaces the previous release's hardcoded "Open dagr" entry on the channels group header, which special-cased one literal action id across six source files: dagr is now just a plugin whose manifest declares a matching context, it appears on every group header rather than only the channels one, and nothing in bora names it. `projects.yml` members may also declare a per-member `template:` that is substituted into `defaults.open_with`'s `<template>` placeholder (e.g. `herdr-plus open web`); when the named opener is not available, opening falls back to bora's own `workspace open` instead of failing. Neither dagr nor herdr-plus is ever a hard dependency.
- A `PULL REQUESTS` band renders under a project: one row per open PR you authored whose head branch has no local worktree, with its number, title, a draft marker, and a CI-status glyph. Clicking a row opens that PR in a new worktree — the same action the right panel's PR right-click already offered, reached through the same code path, so the two cannot diverge. PRs that already have a worktree are deliberately omitted: once a PR is a workspace it is shown as one, so it never appears twice. The band is declarable in `sections.order:` as `pull_requests` like the other four. CI status costs no extra work: the `gh pr list` call that already runs gained one `--json statusCheckRollup` field, rather than correlating with the separate per-workspace checks cache, which is refreshed on its own schedule and could not be made to agree. A row whose repo has no open workspace renders but stays un-clickable, since there would be nothing to name as the new worktree's repo.
- `project.create` now creates or reuses the project's channel workspace (`#<slug>`, or the explicit `channel:`) instead of leaving it a name with nothing behind it — reusing an existing channel never touches its transcript or roster. An agent started (`agent.start`) in a pane whose workspace sits inside one of the project's member directories auto-joins that channel, going through the same `channel.join` path a human would, so it is never double-joined or re-briefed on a second start. `project.update` re-binds rather than renames: changing `channel:` leaves whatever channel the project pointed at before completely alone and only binds the new one. A new `auto_join` field on a project (default `true`) opts every one of its members out of auto-join at once when set to `false`.
- Sidebar workspace view mode is now a three-way `ui.view_mode` setting (`flat` / `repo` / `project`) instead of the old `ui.group_workspaces_by_repo` boolean, cyclable via a header click or `prefix+shift+v` (`Flat -> Repo -> Project -> Flat`) and persisted across restart. `project` renders identically to `repo` for now — its own entry model is a later change. The old boolean config key still works unchanged (`true` -> repo, `false` -> flat).
- The Project view is now a real three-level tree: project → worktree → workspace. A `WorktreeRow` is keyed by the checkout, not the repo, which is the level the Repo view does not have — two worktrees of one repo are two rows, and two workspaces on one checkout share a row. The repo name appears exactly once per row-path (project rows omit it, workspace and pane rows never repeat it), and it collapses out entirely when a project holds a single repo. Members come from `projects.yml`, matched on resolved checkout identity, so a sibling directory whose path merely starts with a member's path is not swept into the project. Workspaces matching no declared member fall into one trailing implicit group. Every row is one line tall, so the three-pass sidebar lockstep contract is unchanged.
- A workspace with two or more panes now renders one child row per pane. A single-pane workspace is exactly one row, as before. This exists because a workspace row's agent label only ever reflects the workspace's *first* pane, so `bora pane split` followed by `bora agent start --pane` produced a second agent with nothing on screen to show it.
- Project rows are clickable and hit-tested from the geometry pass rather than from row arithmetic, so a click still lands correctly when the list is scrolled. Clicking a project, worktree, or section row toggles its collapse; clicking a worktree found on disk with no workspace open on it opens that checkout; clicking a pane row focuses that pane. A right-click assembly menu edits `projects.yml` through the existing `project.*` verbs — add to project, new project, rename, remove — with entries gated on whether the workspace is already a declared member.
- `COMMANDS` and `CHECKS` bands render under a worktree from the project's declared `sections:`, with a right-aligned `running/declared` count and a rule that fills to the counter column. A project that declares no sections renders no bands at all.
- The agent detail panel no longer occupies the bottom of the expanded sidebar: the workspace list takes the whole column, which is the room the Project view's three levels need. Nothing was deleted to do it — every panel helper keeps its call sites and its tests, no `AppState` field was dropped, and a session written by an earlier build (including its `sidebar_section_split`) restores unchanged.
- Worktrees that exist on disk but have no workspace open on them now render as dimmed rows under their project, and the project row's `n/m` finally means something: `m` counts every worktree of the project, `n` only the open ones. The row shape, its dimming, and the click-to-open hit-test had all shipped earlier with nothing able to produce one — the missing piece was the inventory. It arrives from a `git worktree list` per declared repo on a 30-second background refresh (deduplicated per repo identity, one thread, no config key), so the sidebar's render path still performs no git call, no subprocess, and no path canonicalization. Bare and prunable worktrees are skipped, and a worktree that is already open never doubles up as a dimmed row.
- The Project view's right-click menus now edit `projects.yml` for real. Right-clicking a workspace row offers a project section in place of the old visual-group items (which were always no-ops in this view): "Add to \<slug\>" for each declared project, "New projectThe workspace list opened with a `spaces` title on the left and the current view-mode name (`flat`/`repo`/`project`) on the right; both were noise — the title named the obvious and the view name spent a row to answer a question the list below already answers. The row is reclaimed: the group list starts at the sidebar's top edge in every view mode. Cycling views is unchanged where it matters — the `cycle_view_mode` keybind (default `prefix+shift+v`) and the sidebar section of the settings dialog — but the view-mode word was also the click target for cycling, and that affordance leaves with it. The settings dialog's own description of the project view is corrected in the same pass: it claimed project "renders like repo for now", which has been false since the project view shipped." (a name prompt that creates the project with the row's workspace as its first member), or "Remove" when the workspace already belongs to one. Right-clicking a group header — including "Ungrouped" — offers "New project…", "Add workspaces…" (a picker listing exactly the workspaces no project claims, so filing the orphans away never shows you a workspace that is already filed), and "Rename project…" on declared projects. Every write goes through the atomic `projects.yml` update path (or the `project.*` socket verbs from the TUI), and the sidebar picks it up on the next tick — no restart, no hand-editing YAML. The assembly logic was built in an earlier round but never wired to a menu; this change wires it.

### Fixed

- Project view worktree rows always carry the repo name now. The repo column used to collapse away when every worktree in a project belonged to one repo, which read fine while project names named their repos — but project names are arbitrary (`Teste`, `BILLING FIX`), and a bare `main ↑3` under an arbitrary name says nothing about which repo it is without clicking. The column no longer collapses; a two-line worktree card (branch + diffstat over repo · PR · merge state) is the planned follow-up.
- Projects created or edited while the server runs now group in the Project view on the next tick instead of after the next restart. The `projects.yml` store was loaded once at server boot and then polled only by the standalone TUI's scheduled-task loop; the headless server — the process that actually renders the sidebar for every attached client — runs its own trimmed tick that never got the poll, so a right-click `New project…` wrote the file correctly (the verbs always worked) and then nothing moved on screen. The reload decision now lives in one `App::poll_projects_store` helper called by both tick paths, so the two cannot drift again.
- Switching workspace no longer takes several clicks. …Every click was already working — state changed and `workspace.focus` was logged each time — but the screen kept showing the previous workspace, so there was nothing to tell you it had worked. Swapping which workspace fills the terminal area reflows every pane in it without changing the terminal's own width and height, and both transport encoders decide between a full repaint and a diff purely from whether those changed; the diff then ran against content that had already reflowed, leaving the visible terminal out of step until some unrelated redraw happened to fire. Switching tab had the same gap for the same reason. Both now ask for a repaint when, and only when, something actually moved.
- An agent blocked asking you a question no longer reads as background in the agent panel. Every inactive row's state label was dimmed unconditionally, which muted the red `blocked` label — the one thing the panel most needs to say. Rows that want you (blocked, or finished while you were away) keep their full-strength colour; working and idle rows stay dimmed, because that is what dimming is for.
- The aggregate state shown for a workspace or tab was, in one specific case, a function of hash iteration order rather than of state. Folding many panes down to one used a key that ranked several distinct pane states identically — blocked, working, and unknown each rank the same whether or not you have seen them — so `max_by_key` over a `HashMap` of panes broke the tie by returning whichever tied pane iteration happened to reach last. Stable within a run, arbitrary between two. The fold key is total now, and between two otherwise-equal panes the one you have NOT seen wins.
- A CI check with no result no longer displays as a green tick. The checks rollup only ever classified the six hard-failure conclusions explicitly and let everything else fall through to passing, so a `COMPLETED` check whose conclusion is `null` — which GitHub really does emit — read as green, and so did any conclusion string GitHub adds after the code was written. Both are now pending: green is a claim about someone else's CI, pending is an admission that we do not know, and only one of those is safe to be wrong about. An unrecognised `status` behaves the same way. One classifier now owns "this `(status, conclusion)` means this state" for the rollup glyph, the `n/m` count, the per-failing-check rows, and the new PR band, so the four cannot disagree about the same PR on the same screen; the previous code shared only the failing set and left each caller to infer the rest, under a comment promising they "can never drift apart". They had drifted by the time a fourth caller existed.
- A project's `members:` now honour `worktrees: all`, which is their documented default. The Project-view matcher compared resolved checkouts for equality, and a linked worktree's checkout is its own path, so a workspace opened inside a worktree of a declared member directory matched nothing and fell into the trailing `Ungrouped` group — the exact opposite of what `worktrees: all` promises. `all` now matches on repo identity (every checkout of that repo, main and worktrees alike, keeping the member's sub-directory constraint), and `this` keeps the old exact-checkout behaviour.
- The sidebar's collapse toggle was unclickable once the workspace list gained the full column: the list's footer landed on the sidebar's last row, where the right-aligned global launcher overlapped the toggle's cell and, being hit-tested first, swallowed the click. The launcher now stops before that cell, and since rendering reads the same rect, the two move together.
- Chat view timestamps printed a stray digit (`16:111` instead of `16:11`), from a format string interpolating the column-gap constant instead of that many spaces.
- A channel broadcast no longer delivers back to the pane that sent it. The sender was in its own fan-out, so it received its own message and accumulated unread counts for text it wrote itself. Targeted sends (`--to <nick>`) were never affected, which is why the echo looked like it depended on addressing.
- The message a channel delivers into an agent's pane now carries its own sequence id: `[#eng seq=7 from w2:p1 rev] text`. The channel protocol block already told agents to catch up with `channel tail --after <seq>` and to answer a `channel.ask` with `--reply-to <seq>`, but the seq was only ever returned to the *sender*, so a recipient was being told to pass a number it had no way to read. Field order is otherwise unchanged. `CHANNEL_PROTOCOL_VERSION` moves to 3, so every already-briefed pane is re-briefed with the shape it will actually receive.
- The channel protocol briefing now defines what a nick may be, and names the human. It recommended `channel send --to <nick>` without ever saying that a member is addressable by its raw pane id (`w78:p1`), by that id with the colon dropped (`w78p1`, unique even when every other form collides), or by its name — so an agent seeing `name: "omp"` on every row of `channel members` concluded addressing was impossible and fell back to broadcast, the one fan-out width that reaches everybody's context. The briefing also never mentioned that a human is on the channel and addressable, so questions meant for the human were routed through other agents; it now ends with `The human on this channel is @<name>`.
- `--channels` fencing now covers `events_wait`'s nested channel. The fence only checked a top-level channel `name` param, but `events_wait` carries its channel inside `match_event` (`{"event": "channel_message", "channel": ...}`) and the event it returns contains the channel's full traffic — so an MCP server started with `--channels eng` could wait on and read any other channel verbatim, making the `channel_history`/`channel_tail` fence decorative. The nested channel is now rejected with the same out-of-scope error, and a `channel_message` match with a missing or non-string channel fails closed. Non-channel matches (pane/workspace waits) are unaffected, and `events_subscribe` never carried a channel to fence.
- Indented sidebar workspace rows show what is unique to them instead of the repo-derived name, which rendered identically for two workspaces on the same checkout and branch. A child row now carries its `@wNpN` pane badge plus the branch only when the parent header did not already print it; custom-named rows are unchanged, and a pane with no agent identity at all keeps the old display name rather than going blank.
- A project's declared `orchestrator` is now actually launched through `srt` instead of `sandbox::compose_orchestrator_launch` sitting unused: `agent.start` writes the composed `filesystem`/`network` settings JSON into the run file's `.bora/` directory and types the srt-wrapped command line, only for the pane/kind that project names as its orchestrator — an ordinary `agent.start` is never wrapped. The read fence was previously a no-op (`allowRead` alone confines nothing under srt's deny-then-allow read model); it now `denyRead`s the operator's whole home directory and re-allows exactly the member directories, plus a per-member `.env` deny that stays denied even inside that allowed region (a more specific `denyRead` wins over a coarser `allowRead`). The injected instruction no longer promises network research the sandbox's empty `allowedDomains` denies; it now tells the orchestrator to delegate research to a worker over the project channel, the same way it must already delegate edits. `allowUnixSockets` is ignored by srt on Linux (seccomp can't filter by path), so the socket-scoped-to-bora guarantee this fence relies on is macOS-only today.


### Removed
- The sidebar's two-word chrome line is gone. The workspace list opened with a `spaces` title on the left and the current view-mode name (`flat`/`repo`/`project`) on the right; both were noise — the title named the obvious and the view name spent a row to answer a question the list below already answers. The row is reclaimed: the group list starts at the sidebar's top edge in every view mode. Cycling views is unchanged where it matters — the `cycle_view_mode` keybind (default `prefix+shift+v`) and the sidebar section of the settings dialog — but the view-mode word was also the click target for cycling, and that affordance leaves with it. The settings dialog's own description of the project view is corrected in the same pass: it claimed project "renders like repo for now", which has been false since the project view shipped.

## [0.32.0] - 2026-08-20

### Added
- Channels now carry an unread count, so a `channel note` — the zero-injection verb — can sit like mail until someone chooses to read it. `bora channel list` run from an agent's pane reports that agent's own unread per room, `bora channel members` reports it per member, and reading a room via `channel history` or `channel tail` catches that member up. The cursor is persisted per member beside the existing roster and scope records.
- The chat view marks rooms with messages newer than what the window has shown, and ranks them above quiet rooms. This is the window's own view state, not the agents' mailboxes: looking at a room clears its marker for you without touching what any agent still has unread.

## [0.31.0] - 2026-08-20

### Added
- A channel workspace now comes up with two panes: one following the transcript and one plain shell to type `bora channel send` into. The previous single pane was seeded with the transcript follower, which made the room readable but left nowhere to reply from.
- `bora channel open <name>` focuses a channel's workspace and adds whatever the room is missing. Rooms created before the transcript pane existed stayed a bare shell forever with no way to see their traffic; this repairs them, and is a no-op on a room that is already complete.

### Fixed
- The Linux clipboard and notification tests called `.unwrap()` on a `parking_lot` mutex guard, which does not return a `Result`, so the Linux CI leg could not compile the crate's tests. The file is Linux-only, so a macOS `just check` never saw it.

### Synced from herdr
- Merged upstream herdr through `2c042bb2`, which includes the `v0.8.2` release. The fork's reported identity moves to `v0.8.2[2c042bb2]`.
- Unix CLI commands exit quietly when a downstream pipe closes instead of panicking with exit 101, and shell-completion output goes through the same path. (herdr #2994, #2996)
- Busy multi-pane sessions avoid redundant hidden-pane wakeups and full terminal-state formatting in pane-scaled paths, preventing CPU regressions from high-rate background output. (herdr #2550, #2901, #2962)
- Mouse capture and forwarding survive a live handoff, so a reattached client keeps working mouse input.
- Claude screen detection gained activity fallbacks, and Windows recognizes the Cursor CLI's bundled node process.
- Plugin pane working directories stay synchronized with the pane they were opened from. (herdr #2985)

## [0.30.0] - 2026-08-20

### Fixed
- The chat view's channel list is ordered by the last message's timestamp instead of its sequence number. Sequence numbers are monotonic within one channel and mean nothing across channels, so two rooms whose counters happened to agree tied and fell back to the name, ranking a room that had been quiet for two hours above one active minutes ago.

## [0.29.0] - 2026-08-20

### Added
- The chat view's channel list (`prefix+i`) now shows what each channel is worth opening for: pane and agent counts plus the time of the last message, sorted most-recently-active first, with never-messaged rooms dimmed at the bottom. `channel.list` carries the new `last_message_seq`/`last_message_ts` facts; the relative formatting stays in the client.

### Fixed
- The CLI integration tests referenced the upstream binary name, which cargo does not define for this fork, so they could not compile on Linux. Those tests are gated off macOS, so a local `just check` never compiled them and the break only appeared on CI. A guard test now scans for the stale name on every host.

## [0.28.0] - 2026-08-20

### Added
- Plugin tabs now take their name from the manifest pane `title`, so a `placement = "tab"` plugin pane no longer opens as a bare numeric tab and plugins no longer need a follow-up rename call.
- Bundled a new `gitui` plugin example (`examples/bora/plugins/gitui`) that opens gitui in its own tab and auto-opens it on `worktree.created` / `worktree.opened`, installable with `bora plugin link`.

### Fixed
- `bora channel members` now always reports an addressable name. It used to derive the name from only `agent view set`/`agent rename`, while `--to`/`@nick` matching fell back further to the detected tool kind, so a pane addressable as `--to omp` showed up as `name: null`. Both paths now share one fallback chain — registered display name, then `agent rename` name, then detected kind, then the pane's compact addressable id (`w78p1`) — so every member is addressable by something unique and typeable even when three same-kind agents share the room.
- `bora channel --help` no longer hides working commands. The help text is generated from a separate spec that had fallen five commits behind the real parser, so `channel note`, `channel ask`, `send --to`, `send --reply-to`, and `join --scope-write/--scope-read` all worked but appeared nowhere in `--help`, pushing callers onto the in-body `@nick` form that degrades to a broadcast when it does not uniquely resolve. A regression test now asserts the spec covers the dispatcher's real surface.
- `bora channel create` now seeds the new channel's pane with `channel tail --follow`, so a freshly created room shows its transcript instead of a bare shell prompt. The backlog was never lost — only unseeded — and a seed failure logs a warning without failing channel creation.
- A queued when-idle message is no longer replayed into a one-tick agent-status flicker. Agent status comes from screen detection, and coding agents routinely clear their busy indicator for an instant between internal steps; the drain fired on that first transition and landed the message mid-turn. Delivery now waits for the target to stay non-working for a short settle window, and a return to working cancels the replay and keeps the message queued.

## [0.26.0] - 2026-08-19

### Changed
- Agent notification toasts and system notifications now name the pane they came from: the context line appends the pane's public id (`<workspace>:p<number>`, e.g. `w7:p1`) after the workspace label, so a toast shown for a background workspace tells you which split produced it.
- The sidebar row's ` @nome` agent badge no longer falls back to the detected agent kind (`@omp`, `@pi`) when the pane has no registered `agent rename` name. A tool kind names a tool, not an agent; the badge now shows the pane's addressable id instead (`@w78p1`, the unpunctuated form `bora agent prompt`/`orc channel send` accept), so an unnamed agent is still directly addressable straight from the sidebar. A registered name still wins.

## [0.24.0] - 2026-08-19

### Added
- Sidebar workspace rows now show three identity badges after the name: ` @nome` for the pane's registered `agent rename` name (falling back to the detected agent kind), ` #canal` (` +N` for more than one) for a workspace with a pane explicitly joined to a channel elsewhere, and a dim `✓` for a linked worktree that's clean and fully merged into the repo's default branch — safe to close. Channel membership refreshes on the same 2s cadence as git status; the collectible mark piggybacks on the existing git-status refresh pipeline and caches its one expensive call (`git merge-base --is-ancestor`) by `(head_sha, default_branch_sha)`, only re-running it when either moves.

### Synced from herdr
- Merged upstream `herdrdev/herdr` master (36 commits, through `a5c69bea`, upstream v0.8.1).
- Windows is generally available on the stable channel, with `Ctrl+1`–`Ctrl+9` keybindings, PowerShell working-directory sync, verified-local-package updates, ARM64 installer retry while x64 emulation holds the executable, and native support for every agent integration.
- Headless servers use a configurable 120×40 virtual terminal instead of 80×24 when no client is attached.
- Hidden panes no longer flood the server loop with redundant wakeups, and foreground typing no longer waits behind render cadence consumed by hidden-tab output.
- Alternate-screen history reads are faster, and experimental pane graphics support high-DPI direct file frames.
- macOS Chinese IME commits reach panes in report-all mode; Ctrl-click URL openers are reaped on Unix; remote terminal hangups are handled on the client.
- Active sidebar rows stay visible, default active rows stay subtle with a new navigate cursor color, and tab/sidebar clicks survive a stray drag report from the terminal.
- Qwen Code detection uses locale-independent title states with localized confirmation fallbacks; Claude Code half-circle spinner frames are all stripped.
- Prompts to blocked agents are rejected, and new panes wait for shell readiness before input.
- OSC 4 palette query bursts are skipped under WSL.
- CLI help points agents at the plain-text guide, documentation index, and built-in control skill; documentation is published in agent-readable and channel-aware indexes; the plugin marketplace gained trending and new-arrival discovery shelves.

### Changed
- The version identity moved out of the sidebar header (`spaces v0.21.0`) and into the Settings modal, right-aligned in the title bar. It also grew a fork-aware form, `v0.8.1[a5c69bea].bora-24`: the upstream herdr release and the short commit of the upstream tip merged into this fork, followed by this fork's own build number (`bora-<minor>`, or `bora-<minor>.<patch>` when the patch is non-zero) — so a report against a running build names both which herdr it's built on and which bora shipped it, instead of only the bora semver, which upstream syncs don't move. `bora --version` now prints both forms together (`bora 0.24.0 (v0.8.1[a5c69bea].bora-24)`): the plain Cargo.toml semver stays first because release CI's packaging step greps the output for it verbatim.

### Fixed
- A stale `prebuilt/libghostty-vt-<target>.a` (built against an older `vendor/libghostty-vt` tree, e.g. before the modify-other-keys patch) could link silently and SIGBUS at runtime — the auto-detected prebuilt bypass in `build.rs` never checked whether the `.a` still matched the vendored source. It now requires a matching `.vendor-hash` stamp (a deterministic FNV-1a content hash over `vendor/libghostty-vt`'s tracked source paths) sitting next to the `.a`; a missing or mismatched stamp falls back to a from-source build with a `cargo:warning` instead of linking blind. `just prebuilt-ghostty` (`build-libghostty-vt-prebuilt`/`fetch-libghostty-vt`) now regenerates both the `.a` and its stamp together. `LIBGHOSTTY_VT_PREBUILT` (the manual override) is unaffected — whoever sets it owns the consequences.

## [0.23.0] - 2026-08-19

### Added
- `bora agent prompt` carries verified sender attribution. The CLI fills `from_pane` from `$HERDR_PANE_ID` (override with `--from <pane>`, suppress with `--no-from`), and the server prefixes the injected text with `[from <pane> <name>]`. The claimed pane is verified against real OS socket peer credentials: the server captures the caller's PID at accept time (`LOCAL_PEEREPID` on macOS, `SO_PEERCRED` via peer_creds on Linux) and walks the OS process tree to prove the caller descends from that pane's shell; an unverifiable claim degrades to `[from? claimed <pane>]` and a client-smuggled `peer_pid` on the wire is discarded (`#[serde(skip)]`).
- `agent.prompted` event: `bora agent prompt` now emits an `EventKind::AgentPrompted` event (subscribable via the existing `events`/`events.wait` API) whenever it writes text into another pane's terminal, carrying the sender pane (`from_pane_id`, auto-filled by the CLI from `HERDR_PANE_ID` when available), target pane/workspace, and the prompt text (truncated at 4KB). Lets a client render a log of agent-to-agent coordination instead of it being invisible PTY input.
- `bora agent prompt --when-idle [--timeout <ms>]`: delivery gated on agent status. An idle target is injected immediately; a working target defers instead of having bytes land mid-generation as stray keystrokes. Deferred prompts go into a bounded per-target FIFO (cap 32, drop-oldest) drained when the pane's agent status leaves `working`. The response is a first-class receipt (`outcome: injected|deferred`, `queue_position`, `queue_id`), never an error reinterpreted by the caller; every terminal fate of a queued prompt emits an API event (`QueuedPromptDelivered` / `QueuedPromptDropped` with capacity/pane-closed/agent-changed reasons), and a verified sender pane gets a one-line `[bora]` PTY notice on drop (notices are never queued and never recurse). Default behavior without `--when-idle` is unchanged.
- Channel messaging verbs on the existing `#`-workspace convention: `bora channel create|list|send|history|members` (alongside the untouched `channel show|set` update-channel commands). `create` makes a `#name` workspace; `send` appends to a per-channel JSONL transcript at `state_dir()/channels/<name>.jsonl` and delivers `[#name from <pane> <name>] <text>` to every agent pane in the workspace with when-idle deferral, reporting `delivered|deferred|failed` per member; `history` tails the transcript (`--json` for raw lines); `members` lists each member pane with agent status before you send. Transcripts are bounded (10k lines, atomic trim to the newest half), and dropped channel deliveries append an honest `from_name: "bora"` system line to the transcript.
- Loop guard for agent-to-agent prompts: a per-`(from_pane, target_pane)` rate limit (2 s cooldown) rejects rapid ping-pong with error code `agent_prompt_rate_limited`; prompts without `from_pane` (human/orchestrator path) are exempt.
- `bora channel tail <name> [--after SEQ] [--follow|-f] [--json]`: cursor-based tail over a channel's transcript instead of a fixed-window replay. Every `ChannelMessage` now carries a monotonic per-channel `seq` (`persist::channels::next_seq`, survives log rotation; pre-seq history lines default to `seq: 0`), and a new `channel.wait` API method (`{ name, after_seq, timeout_ms }`) returns everything after the cursor plus an honest `gap: true` when rotation dropped messages between the cursor and the oldest retained line — never silently resynced. One-shot `channel tail` snapshots the backlog (`timeout_ms: 0`); `--follow` polls `channel.wait` every 2s and advances the cursor itself, printing a `#gap:` notice to stderr when it detects one. Human output is `SEQ HH:MM name: text`; `--json` prints the raw response per batch.
- Structured `@nick` addressing for `channel send`. The socket API's `ChannelSendParams` gains `to` (nick, agent display/assigned name, or raw pane id, resolved against the channel's member panes) and `in_reply_to` (a replied-to message's `seq`, recorded verbatim on the transcript line, never validated). A unique `to` match delivers to that pane only (transcript still appends, zero injections elsewhere); no match or more than one match errors with `channel_nick_unknown` / `channel_nick_ambiguous` before anything is appended — errors exist only on the structured param. The CLI does not yet expose `--to`/`--reply-to`; addressing from `channel send <name> <text>` instead uses a leading in-body `@nick ` token, which degrades to a literal broadcast (never an error, never a dropped message) whenever it doesn't uniquely resolve. `\@` and `\#` escape to literal `@`/`#` in both the stored and delivered text — one text, not two.
- Native TUI chat view behind `ui.chat_view` (default `false`): a three-column overlay (channels | timeline | members) over the existing `channel.list`/`channel.history`/`channel.members`/`channel.send` API, with live updates pushed on send instead of polled. Opens via `prefix+i` (configurable `keys.chat`) or the `chat` entry in the global menu, both hidden unless the flag is on. `Enter` sends, `Esc` closes, `Tab`/`Shift+Tab` cycle channels, arrow/page keys scroll, `Ctrl+U` clears the composer, and clicking outside the overlay closes it.
- The loop-guard rate limit now also covers `channel send`: a verified sender pane may post to the same channel at most once per 2 s window (`channel_send_rate_limited`), closing the bypass where channel fan-out skipped the per-target prompt limit (delivery passes `from_pane: None` at the prompt layer, so only the direct-prompt path was guarded). Checked after addressing validation — a rejected `to` nick never burns the sender's window — and before the message is appended or assigned a `seq`. Sends without a verified `from_pane` (CLI/human) stay exempt, matching the direct-prompt exemption.
- Explicit channel membership, so an agent that already exists somewhere else can be pulled into a channel instead of having to live in its workspace: `bora channel join <name> [--pane ID]` / `bora channel leave <name> [--pane ID]` (pane defaults to `$HERDR_PANE_ID`, the same mechanism `channel send` uses for `from_pane`), backed by new `channel.join` / `channel.leave` API methods taking `{ name, pane }`. A channel's member set is now the panes in its `#name` workspace (implicit, unchanged) unioned with explicitly joined panes, de-duplicated by canonical public pane id, and every member query is built on that one traversal — `channel.members`, the `channel list` summary counts, `channel send` fan-out, and `@nick`/`to` resolution can no longer disagree about who is in a channel. `channel.members` reports `source: "workspace" | "joined"` per member (`channel members` shows it as a fourth column), so a joined pane is visibly not a resident. Joined membership persists next to the transcript at `state_dir()/channels/<name>.members.json` (atomic temp+rename write) and survives a restart; pane ids that no longer resolve to a live pane are pruned on read. Join is idempotent and honest: joining a pane that already lives in the channel's workspace succeeds with `source: "workspace"` and records nothing, joining twice is a no-op success, an unknown channel errors `channel_not_found`, and an unresolvable pane errors `pane_not_found` before anything is written. Leaving is idempotent too (`removed: false` when there was nothing to drop, including workspace-implicit members, which cannot be removed this way).
- A human seat in every channel, so the person at the keyboard is a participant instead of an anonymous observer. Messages sent from the TUI chat view now carry an identity: `ChannelMessage` gains `from_kind` (`agent`|`human`, `#[serde(default)]` to `agent` so existing transcripts keep parsing) and human lines are stored with an empty `from_pane` and `from_name` set to the effective chat name — `ui.chat_name` when configured, otherwise `$USER`, `$LOGNAME`, or `you`. Authenticity comes from where the request originates, not from a claim on the wire: the chat view sends in-process (`tui.chat.channel_send`) and the `from_human` flag is `#[serde(skip)]`, so a socket client cannot mint a human message, mirroring how `peer_pid` already anchors agent attribution. Agents can now address the human: `@<chat name>` resolves to the seat (case-insensitively) in every channel, appends with `to_human: true`, and delivers to no pane, so a message meant for the person never injects into an agent's prompt; a name shared with an agent is genuine ambiguity and errors with `channel_nick_ambiguous` listing both candidates rather than silently preferring one. In the chat view, human senders render bold+accent, `to_human` lines carry a highlight band across every wrapped line, and a `to_human` message that arrives while the view is closed raises the existing needs-attention toast. Human sends stay exempt from the channel rate limit.
- Channel membership is managed inside the TUI chat view, so recruiting an agent no longer means dropping to `bora channel join` and memorizing a pane id. The members column ends in a clickable `+ add agent` row (`Ctrl+A`) that opens a modal prompt listing every running agent across all workspaces which is not already a member — built from the existing `agent.list` inventory, not a parallel discovery path — each row showing its live status and a shortened working directory so same-named agents in different worktrees stay distinguishable. Typing filters by name or directory, `Up`/`Down` move the highlight, `Enter` joins the highlighted agent through `channel.join`, clicking a row joins it directly, and `Esc` cancels the prompt without closing the view; the members column is re-read on join so the membership is seen landing rather than asserted. Each member row carries an explicit `×` remove control (`channel.leave`) at its right edge, deliberately separate from the rest of the row, which inserts `@<name> ` at the composer cursor instead — addressing a member is a click, and a stray click cannot eject one. A pane living in the channel's own `#name` workspace is a member by construction: `channel.leave` answers `removed: false` and that refusal is what the status line reports, rather than a removal that never happened.
- Channels are created from inside the TUI chat view, so starting a room never means leaving the view for `bora channel create`. The channel column ends in a clickable `+ new channel` row (`Ctrl+N`) that opens a modal prompt for the name; a leading `#` is stripped before the call so a typed `#eng` cannot reach `channel.create` as `##eng`, and an empty or whitespace-only name is a no-op rather than an error. `Enter` creates the channel, reloads the list through `channel.list`, and selects the new room so the human lands in what they just made; `Esc` cancels the prompt without closing the view. A rejected create — a duplicate name, or a workspace that failed to spawn — surfaces the server's own `error.message` on the chat status line and leaves the prompt open with the typed text intact, instead of swallowing the failure or discarding the input. Because a channel is a workspace, a successful create requests a full repaint (the sidebar gains a row and pane content reflows without the outer terminal resizing).
- The chat view opens itself when an agent summons the human: a `to_human` message (a `@<ui.chat_name>` mention) arriving while the view is closed now auto-opens the view on the channel that mentioned you — the "chama o $brandos aqui and the room assembles and opens itself" behaviour — under `ui.chat_open_on_mention` (default `true`; only meaningful with `ui.chat_view` on, which stays default false). Two suppression rules keep it polite, each falling back to today's needs-attention toast: the open never happens while the human typed within the last 3 seconds (a `human_last_input_at` Instant written once per keystroke on both the local and attached-client input paths — never in a per-pane or per-render loop), and it never happens outside an explicit quiet-mode allowlist (`Terminal`/`Navigate`), so onboarding, prompts, and modals are never hijacked and any mode added later defaults to not interrupting. The open selects the mentioning channel by name through the same `channel.list`/history fetch the manual open uses; if the channel is not in the refreshed list the toast fires instead of opening on an arbitrary channel. Closing an auto-opened view (`Esc` or click-outside) returns to the exact mode it interrupted — the auto-open records the prior mode; manual opens keep the existing leave-modal behaviour. `to_human: false` chatter remains toast-free and open-free, unchanged.
- Added a `github.pr_opened` event, emitted once when bora successfully opens a pull request for a worktree branch. Unlike the periodic `github.prs_refreshed` poll, it fires immediately on creation and carries the `branch`, PR `url`, and affected `workspace_ids`. It is available to both `events.subscribe` and plugin manifest `[[events]]` hooks. It covers PRs opened by bora itself; a PR opened by running `gh` inside an agent pane does not trigger it.
- CI now runs a deterministic rules review on every push to a PR (`.github/workflows/independent-review.yml`, driven by `scripts/review_rules.py`): it diffs `base...head` so it reviews the pushed commit, checks it against four diff-scoped AGENTS.md rules (version bump, generated/published paths, `#[allow]` justification, issue-closing keywords) with no model call and no credential, and fails the job both when the reviewer can't run and when it finds a violation. Findings block merge, replacing the retired model-based independent review, whose findings were advisory only.
- Bora now teaches channel usage inline: any pane that joins a `#channel` or receives its first message in one gets a one-time `[bora] channel protocol for #name (v1):` prompt covering reply, `@nick`/`--to` addressing, and `channel tail --after`. Delivery reuses the `agent prompt` path (`App::send_channel_protocol`) with `when_idle: true` and no `from_pane`, so it's deferred while the target is working and exempt from the per-sender rate limit; a `(channel, pane, version)` record at `channels/<name>.protocol.json` (`persist::channels::{read,mark}_protocol_sent`) means a server restart never repeats it, while bumping `CHANNEL_PROTOCOL_VERSION` re-sends to panes that saw an older version. Each send appends a `from_name: "bora"` system line to the channel transcript.
- `bora channel send <name> <text> --to NICK`: the CLI now exposes the structured `to` addressing that `ChannelSendParams` already carried, instead of only the in-body `@nick` token. Resolves the same way as `@nick` against a member's display name, assigned name, detected agent kind, or raw pane id, but fails loudly with `channel_nick_unknown` / `channel_nick_ambiguous` on a miss instead of degrading to broadcast.
- `ui.group_workspaces_by_repo` (default `true`, new "sidebar" tab in settings): turn off repo grouping in the sidebar to get a flat workspace list that can be freely drag-reordered — every row, including linked worktrees that grouped mode keeps pinned inside their bracket, becomes an independent drag target and the drop point lands between any two rows. Flat mode dissolves all sidebar grouping (repo brackets, the channels group, visual groups); re-enabling regroups automatically from the same underlying order, so a flat-mode reshuffle shows up as the new member order inside each repo group. The toggle persists in config through the same settings flow as every other boolean, workspaces hidden individually stay hidden in flat mode (a repo hidden at group level has no header off of which to stay hidden, so its rows reappear until grouping returns), and grouped mode's block-drag (moving a whole repo bracket at once) is untouched. The settings popup widens 76 -> 80 columns so the seventh tab no longer clips.
- `channel.send` burst damper: a per-channel sliding window (`ui.channel_burst_messages`, default 8, within `ui.channel_burst_window_secs`, default 600) mirrors orc's `ORC_BURST_N`/`ORC_BURST_MIN`, which the existing 2s per-(pane, channel) rate limit does not cover — a measured 2026-08-14 storm (198 msgs, 5 agents) passed that cooldown easily. Once a channel crosses the threshold, `channel.send` keeps appending to the transcript and emitting `ChannelMessage` events exactly as before, but stops bell-ing member panes: the `agent.prompt` fan-out and the one-time protocol briefing are skipped entirely for the rest of the burst. The transition into burst appends a single honest `[bora]` system line (`canal em surto (N msgs em Ws): gravando sem sino`) — edge-triggered, so a storm never doubles the transcript with one line per suppressed message. The socket response's `ChannelSent` result gains `suppressed: bool` so a caller (and the CLI, which now prints a stderr note) can tell recorded-but-unrung apart from delivered. The internal send path also grows a private `force_bell` flag, always `false` from `channel.send` today, that will let the upcoming `ask`/`hold`/`resume` verbs (`orchestrator-dtq.2`) pierce an active burst without a new wire parameter. `0` on either config key disables the damper. In-memory only; resets on restart.
- `bora channel note <name> <text>` and `bora channel ask <name> <nick> <text> [--timeout MS]`: two new channel verbs, alongside `send`, following orc's insight that fan-out width *is* the verb (`bin/orc:497-670`). `channel.note` is the cheapest: an append-only transcript record with ZERO injection — no `agent.prompt` fan-out, and unlike `send` it is never suppressed by (or recorded into) the burst damper, since there is no bell for the damper to cut. `channel.ask` is the opposite end: exactly one bell, and the caller blocks server-side until the addressee replies or `timeout_ms` elapses (default 300_000ms, capped at 600_000ms) — reusing `channel.wait`'s connection-thread poll pattern rather than the App's own request loop, so a multi-minute ask never stalls other API calls. `channel.ask` delegates its append+inject half straight to `channel.send`'s existing single-target path (`force_bell: true`), so addressing errors, attribution, and delivery classification are identical to a targeted `send`. Correlation is exact instead of orc's fragile `<nick>`-prefix polling (`bin/orc:797-849`): the question's monotonic `seq` (now echoed on every `ChannelSent`/`ChannelAsked` response) is the key a reply threads back through `in_reply_to`, which the CLI now exposes as `bora channel send <name> <text> --reply-to SEQ`. The server rejects a reply pointed at a seq past the channel's current max (`channel_reply_unknown_seq`) — a seq lost to rotation is still accepted, since history is allowed to be gone but the future is not. The channel protocol briefing gains one line teaching `--reply-to` (`CHANNEL_PROTOCOL_VERSION` stays at 1).
- Per-pane directory scope on channel join, backed by a `state_dir()/channels/<name>.scope.json` sidecar (`persist::channels::{read,upsert,remove}_channel_scope*`): `bora channel join <name> [--pane ID] [--scope-write DIR]... [--scope-read DIR[,DIR]]...` records which directories a pane may write (write implies read) and which it may only read, keyed by canonical public pane id — never `terminal_id`, which is minted at runtime and reallocated on restore. `ChannelJoinParams` gains `scope_write`/`scope_read`; re-joining with a new scope replaces the pane's entry wholesale rather than merging, and `channel.leave` always drops the scope entry along with membership. `CHANNEL_PROTOCOL_VERSION` bumps to 2: a pane with a recorded scope entry gets a briefing suffix naming its write/read directories and instructing that anything outside them goes through `@nick` in the channel instead of being touched directly; a pane with no scope entry sees no change. This is the T1 (persuasion) layer of the scope contract in `CANAL-ESCOPO.md`; the harness-side enforcement gate is separate follow-up work.
- `bora mcp serve [--channels a,b] [--nick NAME] [--allow-prompt]`: an MCP server (stdio, JSON-RPC 2.0) exposing a fenced slice of the socket API as tools, for registering bora with an MCP-client harness like OMP. Tools are generated from `Method`'s derived JSON Schema (`schemars::schema_for!`) instead of hand-listed, so a future socket API verb needs zero changes here to appear as a tool once it's added to the allowlist in `src/mcp/tools.rs`. Exposes `channel_list`, `channel_members`, `channel_history`, `channel_tail` (calls `channel.wait`), `channel_send`, `channel_join`, `channel_leave`, and `agent_list`; `agent_prompt` only exists in `tools/list`/`tools/call` with `--allow-prompt`. `--channels a,b` fences every channel-scoped tool to those names before the request reaches the socket, and filters `channel_list`'s result to them. `--nick` is informational, reported in `initialize`'s `serverInfo`. MCP is client-initiated and never wakes an idle agent — that stays `bora agent prompt --when-idle`'s job. Example `.omp/mcp.json` stanza: `{"mcpServers": {"bora": {"command": "bora", "args": ["mcp", "serve", "--channels", "eng"]}}}`.

### Fixed
- Sidebar and right-panel toggle no longer leaves the terminal flickering (content painted a few rows off, alternating with the correct state) until an unrelated full redraw happens to fire. The toggle reflows every pane's column width without changing the outer terminal's size, so neither transport encoding noticed: the default `SemanticFrame` client encoder and the `terminal-ansi`/`--remote` `BlitEncoder` both decided full-vs-diff repaint purely from outer-frame dimensions, so a layout change alone never triggered a full repaint and the diff/scroll-shift path ran against already-reflowed content. `FrameData` now carries an explicit `force_full_repaint` signal (protocol version bumped 20 -> 21) that the server sets on any `AppState`-level layout change and both client encoders honor.
- `bora agent read|prompt|get|explain|focus|rename|send-keys|wait <target>` accept a `terminal_id` (`term_…`), matching what the `<bora-mentions>` context block injected into agents already documents. Only `resolve_agent_target` rejected it, so every command the mentions block told an agent to copy failed with `agent_not_found` while the same string worked for non-agent pane commands (`resolve_terminal_target` accepted it all along). Note that `terminal_id` is stable only within a single server process: it is minted by `TerminalId::alloc()` and never persisted, so both a cold restart and `--handoff` mint a new one. For an address that survives a restart, name the agent with `bora agent rename` and target that name, which is persisted and restored.
- Public pane ids resolve with or without their colon, so `w2Ap1` works everywhere `w2A:p1` does, for both `bora agent …` and `bora pane …` targets. Consumers that cannot carry a colon have to strip it — the orchestrator's channel nick does, because its `@mention` parser would otherwise swallow `:` out of ordinary prose — and the stripped form was then unresolvable, which made a nick unusable as the address it looked like. Fixed in `parse_pane_id`/`parse_current_public_pane_id` rather than in the CLI, since the latter compared the canonical id against the raw input for exact equality and would have kept rejecting agent targets even with the CLI normalizing.
- `github.pr_opened` and the post-PR checks refresh it triggers are now scoped to the repository, not just the branch name: `WorktreeOpenPrFinished`/`GithubPrOpened` now carry `repo_identity`, and `workspace_ids_on_branch` matches on branch *and* `repo_identity` instead of branch alone. With several repositories open at once, a branch name shared between them (e.g. `main`) could refresh — and be announced in the event payload against — a workspace belonging to an unrelated repository.
- Cycling workspaces (`next_workspace`/`previous_workspace`, e.g. `cmd+shift+]`/`cmd+shift+[`) and numbered workspace switching no longer skip a worktree workspace whose branch has exactly one member: the sidebar folds that workspace's row into its `BranchHeader` instead of emitting a separate `Workspace` entry (so the header itself renders and clicks like a workspace card), but `visible_workspace_order`'s `filter_map` only read `WorkspaceListEntry::Workspace`, silently dropping the folded workspace and everything it carried. `visible_workspace_order` now also reads the `ws_idx` folded into `BranchHeader`.

## [0.14.4] - 2026-08-13

### Fixed
- Typed keys are no longer lost when a busy agent pane stops draining its input: keystrokes that hit a full input channel are now queued in order and delivered once capacity frees up, instead of being silently dropped (typing `1234567890` into a busy pane used to yield only a few of the characters).
- `just release-prepare` no longer overwrites `docs/next/CHANGELOG.md` with root `CHANGELOG.md`'s content, which could silently destroy staged unreleased entries; the flow now promotes `docs/next/CHANGELOG.md` (the documented staging file) into root instead, and `just release-docs-check`/`scripts/changelog.py check-history-sync` fails loudly instead of releasing when the two files have diverged.
- `just lint` now prints a reminder listing any touched files that are entirely gated off on macOS (`#![cfg(not(target_os = "macos"))]`, e.g. `tests/auto_detect.rs`), since a green macOS `just check` cannot prove those files are clean — only CI's `ubuntu-latest` leg lints them.
- The retained (incremental) render fast path no longer disables itself the moment a second client attaches. It previously required exactly one connected App/TerminalAttach client and fell back to a full render for every frame otherwise (measured: ~53% of retained attempts falling back with two clients attached); it now serves any number of caught-up App clients from the same computed frame patch.
- `install.sh` reports the real exit code when the freshly installed binary fails its smoke run. `$?` was read inside `if ! cmd`, where it is the status of the negation and therefore always `0`, so every failure printed `exit 0` and the branch that diagnoses a macOS code-signature SIGKILL (exit 137) could never be reached.

## [0.14.3] - 2026-08-13

### Added
- Sidebar "Programs" launcher: a fixed band above the sidebar footer lists each pane-mode `.bora.toml` `[[commands]]` entry for the active workspace's branch, plus an always-on "+ run command…" row that opens a free-text prompt. Clicking an entry spawns it as a center-workspace pane through the existing command pipeline, so external tools (Helix, `gitui`, `bd`) are one click away instead of only reachable from the workspace context menu.

### Fixed
- `website/latest.json` validation now expects the fork's `bora-<target>` release asset names instead of upstream's `herdr-<target>`, in both the current-release and archived-release checks.
- The release-manifest test helper now builds `bora-<target>` asset fixtures, matching `scripts/changelog.py`'s expected asset names.
- Direct installs verify the downloaded binary's SHA-256 against the release manifest again; the checksum comparison had been dropped from `website/install.sh`.
- omp agent state is read from its OSC title (`π > `, `π <spinner> `, `π ! `) instead of a `π  /` body marker omp no longer renders. Every omp pane used to report `idle` through the known-agent fallback, even mid-turn.
- Sidebar idle-age labels (`42s`, `12m`) keep counting while nothing else redraws: an idle pane now arms a 1 s re-render tick instead of relying on the spinner-only animation timer.
- `just bench-render-scale` builds again: the recipe still passed upstream's `--bin herdr` after the fork renamed the binary to `bora`.
- Sidebar spinner animation ticks every 80 ms instead of every 16 ms. The old cadence forced a full app re-render 60 times a second for as long as any pane in any workspace was working, which starved input handling (single keypresses got dropped) and made the outer terminal flicker. omp 17.3.0 rewrites its OSC title spinner every 80 ms, so bora now detects a working pane for essentially the whole turn and paid that 60 fps cost continuously. `SPINNER_TICK_STEP` keeps the visible glyph cadence unchanged.

### Synced from herdr
- Merged upstream `herdrdev/herdr` master (49 commits) into the fork.
- Per-pane right-click routing: panes can forward normal right-click gestures to mouse-reporting applications via the pane context menu, `bora pane input --right-click pane`, or `pane split --right-click pane`.
- Configurable right-aligned tab bar status entries (zoom state, hostname, date/time, literal text, and asynchronously refreshed command output).
- The outer terminal window title now tracks the session through `ui.window_title`.
- New `keys.move_tab_previous` / `keys.move_tab_next` and `keys.resize_pane_*` keybind actions.
- `pane read` and `pane wait-output` now accept `--flag=value` and reordered options.
- Pixel-precision mouse position forwarding for panes that request SGR-pixels reporting.
- Server shutdown requests are prioritized over pane and API traffic.
- Windows: lower idle agent-detection CPU, atomic installer swaps, and native support for all agent integrations.
- Merged upstream `herdrdev/herdr` master again (12 commits): large terminal redraws are compacted instead of skipped, shifted-punctuation keybinds are disambiguated, the scrollback editor preserves logical lines, Claude title spinner frames are stripped, and repeated Git config reads are avoided.
- Added Qwen Code detection for idle, working, and user-confirmation states, plus optional native session restore.


### Setup local (plugins e atalhos) — 2026-08-11

Plugins instalados no bora desta máquina, com auditoria de segurança em
`~/Sites/herdr-plugins/` (clones pinados nos commits instalados):

- **reviewr** (`persiyanov.reviewr`, SAFE) — pane de code-review ao lado do
  agente: diff local, comentários por linha, envio pro input do agente.
  - Abrir/fechar: `ctrl+alt+r` (toggle).
  - No pane: `v` seleciona linhas, `c` comenta, `s` envia tudo pro agente,
    `1/2/3` abas Changes / All files / PR (PR é read-only), `?` ajuda.
- **gh-pr** (`wyattjoh/herdr-plugin-gh-pr`, SAFE) — status do PR + CI na
  sidebar (`#123 ✓/✗/●`), refresh automático a cada 30s por pane.
  - Sem comando: aparece na row do agente (token `$pr` na config da sidebar).
  - Refresh manual: `bora plugin action invoke gh-pr.refresh`;
    abrir PR no browser: `bora plugin action invoke gh-pr.open-pr`.
- **automations** (fork local `~/Sites/herdr-plugins/herdr-automations`,
  linkado; safe-with-caveats mitigado por build do source) — cron de agentes:
  agenda um prompt, o bora acorda um agente num worktree novo.
  - Board: `prefix+a` (overlay); `r` roda agora, `e` edita o YAML, `enter`
    pula pro workspace do último run.
  - CLI: `herdr-automations add | list | run <nome> | history | fire <evento>`.
  - Config: `bora plugin config-dir dnzzl.automations` → `automations.yaml`.
  - **Event triggers (feature nossa, branch `event-triggers`)**: campo
    `on: worktree.created` no YAML dispara a automation quando o evento
    ocorre no repo dela; outros eventos pedem um bloco `[[events]]` no
    manifest. Repo privado: `aryrabelo/herdr-automations`.
- **dashboard** (`chouxcreams.herdr-dashboard`) — TUI com o PR de cada pane
  agrupado por workspace: estado, CI, reviews; daemon coleta a cada 90s.
  - Abrir: `bora plugin action invoke open --plugin chouxcreams.herdr-dashboard`.
  - Scriptável: `herdr-dashboard --once --json` (base para automações de CI).
- **beads popover** (`hexsprite/herdr-beads`, safe-with-caveats /tmp) —
  ctrl+click num id de bead (`https://bead.invalid/<id>`) abre os detalhes
  num split; ids dentro do split também são clicáveis (anda a árvore de
  dependências). Requer `bd` no PATH.
- **file-viewer** (`smarzban/herdr-file-viewer`, safe-with-caveats) — visor de
  arquivo git-aware num split. `prefix+f` abre.
  - Caveat: update-checker liga sozinho e busca conteúdo remoto do GitHub
    (`update_check` no config); desligável.
- **board** (`bredebjorhovd/herdr-board`, ⚠️ HIGH finding, **desabilitado**
  em 2026-08-17 — configurado, não mais inerte) — board global de issues
  GitHub/Linear → dispatch de agentes → review volta pro agente que abriu o PR.
  - **NÃO reabilitar sem ler o achado abaixo primeiro.**
    Reportado upstream: https://github.com/bredebjorhovd/herdr-board/issues/49
  - `review.rs`: comentário de PR de **qualquer conta com acesso ao repo**
    é digitado automaticamente no pane do agente vivo como se fosse
    instrução — sem checar autor. `dispatch.rs`: título/corpo da issue vai
    verbatim pro prompt de abertura, sem delimitador.
  - Config real desde 2026-08-13: `.env`/`routing.toml` em
    `~/.config/bora/plugins/config/board/` — `routing.toml` é symlink para
    `~/Sites/orchestrator/board-routing.toml` (versionado lá, branch
    `board-deliver-reviews-false`), 4 repos privados do `postpilot-org`,
    `pull_requests`/`writeback` já `false`. Faltava a mitigação real —
    `doctor` mostrou `review delivery: on` por default mesmo com as outras
    duas off, porque é uma chave própria — agora coberta por
    `deliver_reviews = false` no routing.toml.
  - Se/quando reabilitar: `bora plugin enable board`; toggle
    `prefix+shift+o`; confirme com `herdr-board doctor` que `review
    delivery` segue `off` antes de tirar o pé.

Removido: `tam.pr-workflow` (prompt de merge automático indesejado; fork
patchado ficou em `~/Sites/herdr-pr-workflow`).

Keybinds versionados em `dotfiles-2026/dotfiles/bora/config.toml`.

## [0.13.2] - 2026-08-05

### Added
- Windows clients can now remote attach to unix hosts. (#2329)

### Changed
- `theme.custom.sidebar_bg` can now give the desktop sidebar its own background without changing built-in theme defaults.

### Fixed

- Configs containing the retired Herdr-written `ui.agent_panel_scope` setting no longer report it as an unknown key after upgrades. (#2292)
- `pane query --current` now resolves the calling pane correctly instead of an unrelated one. (#2298, refs #2297)
- Default mouse reports, including ones split across reads, now parse correctly instead of being dropped. (#2312, refs #2309)
- The tab navigator now also searches single-tab names, not just multi-tab groups. (#2320)
- Closing a pane now returns focus to the pane its split was opened from, instead of an unrelated neighbour in tree order. (#2266)
- Halfwidth katakana voiced sound marks (e.g. `ｶﾞ`) now render correctly instead of the mark corrupting the following character. (#2257)
- `modifyOtherKeys` key releases are now preserved instead of being dropped. (#2303, refs #2302)
- The collapsed sidebar now highlights the focused agent pane, matching the workspace list and expanded panel. (#2382)
- Ctrl-Tab no longer sends stray escape sequences to legacy (non-kitty-keyboard-protocol) panes. (refs #2296)
- OSC 4 palette overrides are now rendered directly instead of being forwarded by index, fixing incorrect colors. (#2162)
- Plugin marketplace counts now stay current instead of going stale.

## [0.13.1] - 2026-08-04

### Added
- Added `ui.tab_bar_position = "bottom"` to place the desktop tab row below terminal panes.
- Copy mode now supports literal smart-case search with `/` and `?`, repeating with `n` and `N`, match highlighting, and tmux-style cross-line `w`/`b`/`e` word motions. (#1230)
- Added maki detection with idle, working, and blocked screen states. (#1301, thanks @tontinton)
- **Pull Requests tab** in the right panel. A new "PRs" tab (alongside Changes / Checks / Issues) lists the current user's open PRs for the active workspace's repo, with mergeable indicators (`✓` green = MERGEABLE, `✗` red = CONFLICTING) and draft markers. Clicking a PR row opens its context menu (Open in worktree / Open in browser / Copy URL). The PR list is refreshed on tab open and periodically in the background.
- Create worktree modal opened by a "+" button on each repo header row, with GitHub / Branch / Name tabs. GitHub lists the repo's open pull requests and issues: a pull request opens its worktree, an issue runs the configured `[flow]` command (issue rows are disabled with a hint when no `[flow]` command is set). Branch checks out an existing local branch; Name creates a fresh branch. The existing `new_worktree` keybind and the `GitWorkspace` context-menu "New worktree" entry open the same modal.
- Added `github.pulls.list` and `github.issues.list` socket API methods to read cached open pull requests and issues per repo, plus `github.prs_refreshed` and `github.issues_refreshed` events.

### Changed
- Idle, not-yet-seen panes now show an animated braille "sand" glyph whose color ramps gray to red by idle age, and working panes show an animated spinner; the animation timer only schedules redraws while an animation is actually visible.
- Pull request rows have been removed from the left sidebar. PRs are now managed exclusively through the right panel's "PRs" tab, which provides a cleaner dedicated surface with mergeable status, draft markers, and the same context-menu actions (Open in worktree / browser / copy URL). The Create-worktree modal's GitHub tab continues to offer a separate path to open a PR worktree.
- Settings and `ui.status_indicators = "symbols"` can now use distinct static shapes for blocked, working, done, idle, and unknown agent states. (#2260)
- The plugin marketplace now discovers valid manifests at repository roots and subdirectories, groups multiple plugins under each repository, and publishes their versions and exact default-branch commits.

### Fixed
- Claude Code confirmation prompts using `Enter to confirm · Esc to cancel` now report `blocked` instead of `idle`. (#2268)
- Sidebar agent lists keep scrolling when differently sized clients are attached to the same session. (#2255, thanks @aiworkflowpro)
- `pane send-keys` and `agent send-keys` now preserve Shift when sending `shift+tab`, allowing agent permission modes to be cycled programmatically. (#1561, thanks @keinstn and @tomohisa)

## [0.8.0] - 2026-08-03

### Added
- Added `herdr --skill` to print the agent skill bundled with the running Herdr binary.
- Added `ui.pane_scrollbars = false` to hide terminal pane scrollbars and reclaim their reserved column. (#2167)
- Added `ui.tab_bar_position = "bottom"` to place the desktop tab row below terminal panes. (#2117)
- Added live filtering to the keybind help with `/`, Backspace, and `Ctrl+U`. (#1825, #1832, thanks @corrius)
- Added Windows support for `experimental.switch_ascii_input_source_in_prefix` with Korean IMEs. (#1802, #1823, thanks @joonhwan)
- Added Grok CLI session reporting and native restore with `grok --resume <id>`. (#1800, #1807, thanks @carlesso)
- Added Antigravity CLI session reporting and native restore with `agy --conversation <id>`. (#1011, #1571, #2087, thanks @ludoo)
- Added automatic text history reads for idle alternate-screen agents, with the application viewport restored after collection.
- Added `workspace.move_block`, the `workspace.reordered` event, and atomic worktree-group reordering. (#1694)
- Added a Simplified Chinese README. (#1990, thanks @patrick-xin)

### Changed
- Experimental options are no longer exposed in the Settings TUI and remain available through the config file.
- Agent status indicators now use the same static workspace marks across the sidebar, navigator, and mobile views, eliminating continuous spinner rendering while agents work.
- Hidden pane output no longer triggers unnecessary TUI rendering.
- Windows preview downloads now include Herdr and a modern app-local ConPTY runtime in one archive. (#1533, #1644, #1828)
- Worktree parents and children now stay packed together in the sidebar, including while groups are reordered.
- Public documentation now separates stable, preview, and immutable versioned release snapshots.
- Repository and installation links now use `herdrdev/herdr` after the GitHub organization migration.
- Relicensed Herdr from AGPL-3.0-or-later to Apache-2.0.

### Fixed
- Pane applications now receive semantic light/dark query responses and live Mode 2031 updates when the host appearance changes. (#714)
- Remote attach now falls back to `sh` when the login shell cannot perform path discovery. (#1201)
- PTY output continues to be read while pane input is temporarily blocked. (#1295)
- Worktree CLI help and docs no longer advertise the redundant `--json` flag; worktree commands remain JSON-only and continue accepting the flag for compatibility. (#2171)
- OpenCode 2 preview panes now appear as OpenCode agents and use the existing OpenCode status detection. (#2169)
- Pane text copied through VS Code Remote Tunnels now reaches the viewing machine's clipboard instead of overwriting the remote host clipboard. (#2015)
- Windows agent detection now follows Git Bash-launched agents across emulated `exec` process boundaries. (#2107)
- Detached Windows servers and pane processes now survive logout from the OpenSSH session that started them. (#2008)
- Windows `agent start` now launches agents without native arguments instead of timing out on an invalid empty PowerShell argument list. (#2072)
- Headless servers now resume restored agent sessions without waiting for a TUI client to attach. (#2064)
- Vibe and other Kitty-keyboard pane applications now receive shifted letters and punctuation when they request associated text. (#2020)
- Kitty-keyboard pane applications now receive printable key releases without duplicate text input. (#1746)
- Kitty graphics remain visible during host repaints. (#1628)
- Pane applications now receive correct XTWINOPS terminal and cell-size query responses. (#835)
- WSL clients query the host cell size when the terminal ioctl reports no pixels, keeping graphics sharp instead of using the 8x16 fallback. (#2146, #2160, thanks @WakaTaira)
- Linux runtimes without terminal foreground process groups can opt into child-group agent detection with `HERDR_PROCESS_DETECTION=child-groups`. (#1982)
- Installing the Herdr agent skill with the `skills` CLI no longer copies the entire repository. (#2022)
- Nix builds now include the bundled agent skill required by `herdr --skill`. (#1889, #1890, thanks @olafkfreund)
- Agent prompts now wait briefly after sending text before pressing Enter, preventing prompts from remaining in agent composers without starting a turn. (#1878)
- Empty clipboard writes from pane applications no longer erase existing clipboard contents or show a copied confirmation. (#1893)
- Plain mouse movement no longer triggers continuous full renders while preserving Herdr menu hover and pane application mouse tracking. (#1865)
- Extended-button drags now preserve Herdr hover state while applications receive the drag.
- `ui.copy_on_select = false` now retains drag and double-click word selections without copying; `Ctrl+C`, or `Cmd+C` when the host terminal forwards it, copies and clears the selection. (#1782)
- Pane and agent read responses now report `truncated: true` when older terminal rows were omitted. (#1717)
- Pane applications that query OSC 4 palette colors now inherit the host terminal palette. (#1752)
- Ctrl-clicking a pane URL no longer forwards an unmatched mouse release to alternate-screen applications, preventing duplicate browser tabs. (#1761)
- Known-agent integrations now leave pane ownership to confirmed process exit, so restarting Pi with the same saved session restores lifecycle state even with custom working UI. (#1648, #1792)
- Nested or ephemeral Codex sessions no longer replace the owning pane's resumable session. (#1789, #1927, thanks @Pimpmuckl)
- Pi RPC, JSON, and print processes no longer claim pane lifecycle state intended for Pi TUI sessions. (#2159, thanks @rhjoh)
- Hermes state now comes from screen detection while its plugin reports resumable session identity, avoiding stale lifecycle authority from incomplete hooks.
- OMP integration install, status, and uninstall now respect `PI_CONFIG_DIR` when `PI_CODING_AGENT_DIR` is not set, and installation refuses extension-directory collisions with Pi. (#1696)
- OMP integrations now preserve Windows absolute session paths for native restore. (#2092, thanks @art-wiedzmin)
- Claude integration updates preserve existing settings key order and formatting. (#2066)
- Physical Escape key records on native Windows now bypass raw VT report framing, so pane applications receive Escape immediately and reliably. (#1736)
- Native Windows key presses, grouped repeats, and releases now preserve their physical lifecycle and stay with the pane that received the initial press. (#2077)
- Windows `pane send-keys` and `agent send-keys` now deliver semantic Escape as a complete key tap, preventing a following key from being interpreted as an Alt chord.
- Shift+Enter now reaches native Windows pane applications with its modifier intact. (#1743, #1909, thanks @Pimpmuckl)
- Ctrl+_ input bytes now decode as Ctrl+_ instead of Ctrl+-. (#2164, #2165, thanks @Sertug17)
- Prefix and navigate modes now recognize non-US shifted keybindings while retaining legacy US punctuation support. (#1870)
- Closing a non-focused workspace no longer changes the focused workspace. (#1328, #1877, thanks @yianL)
- A background workspace that closes after its last pane exits no longer moves focus or hides the current workspace. (#1621, #1912, thanks @season179)
- Directional pane focus now keeps Navigate mode active. (#1850, #1993, thanks @we11adam)
- Closing a workspace's last tab through the CLI or API now closes the workspace like the TUI does. (#1760, #1899, thanks @season179)
- Linked worktree workspaces retain their labels during Git metadata refreshes.
- Clients repaint after transient terminal resizes instead of leaving stale or missing rows.
- Repeated workspace Git discovery and foreground-cwd checks no longer block rendering or API handling. (#1838, #2206)
- Relative plugin commands now resolve from the plugin root. (#1949)
- Windows installation preserves inherited `PATH` and related environment variables. (#1947)
- Windows agent process discovery preserves the owning parent agent across wrapper processes. (#1514)
- The Rose Pine `surface_dim` color remains visible when the outer terminal uses a matching theme. (#1946, #2002, thanks @brabli)
- CLI socket commands now report a clear `server_not_running` error instead of a raw I/O error. (#1941, #1963, thanks @season179)
- Non-UTF-8 CLI arguments now produce a usage error instead of panicking. (#2207, thanks @VialFlorian)
- Copy-mode `e` now crosses long soft-wrapped CJK lines when a read window ends on a wide glyph. (#2145, thanks @kiakiraki)
- Clients restore terminal state when they receive SIGHUP or SIGTERM. (#2041, thanks @MattJColes)
- Windows now shows `system` notifications and completes MP3 notification sounds without leaving PowerShell players waiting for a timeout. (#1330)

## [0.7.5] - 2026-07-21

### Breaking Changes
- Installed and linked plugins, including their enabled state, are now global to the current user instead of isolated by Herdr session. Plugins installed only in a named session on Herdr 0.7.3 must be installed or linked again. (#1174)

### Added
- Added a live-agent CLI facade with named `start`, atomic `prompt`, logical `send-keys`, and server-owned `wait` workflows. Agent startup targets an existing pane without changing topology, validates the requested interactive agent kind and strict agent name, and accepts native arguments after `--`.
- Added transient declarative Agent view queries through `agent.view.set/clear`; filtered and sorted views now define sidebar, mobile, mouse, and agent-keybind navigation order.
- Added one-shot plugin `[[startup]]` hooks for restoring plugin-owned state after server startup and live handoff.
- Added per-token foreground, bold, and dim styling to expanded Space and Agent sidebar row layouts.
- Added `ui.sidebar_start_collapsed` to launch Herdr with the sidebar collapsed. (#1463)
- Added `ui.prompt_new_workspace_name` to ask for a workspace name before interactive TUI creation.
- Added macOS support for the `HERDR_AGENT=<agent>` foreground-process hint, allowing agents hidden behind host-visible wrappers such as `nono` to use the named agent's screen manifest. (#679)

### Changed
- Agent commands now accept only a unique live agent name or the pane ID currently hosting that agent. Names are cleared when the occupant exits, is released, or is replaced. The old top-level `wait` commands were replaced by `agent wait` and `pane wait-output`, and `agent send` was replaced by `agent send-keys`.
- The session navigator now uses connected tree glyphs, groups matches by workspace, and automatically selects the first result when a search begins. (#1611)

### Fixed
- CLI requests now return a machine-readable `protocol_mismatch` error when the client and server protocols differ, while recovery commands remain available. (#1435)
- Linux sound notifications now terminate and reap audio players that do not exit, preventing unavailable audio from leaving CPU-bound `mpg123` processes behind. (#1622)
- Oversized bracketed text pastes are now rejected with a client-local notification instead of disconnecting the client. (#1665)
- Agent prompt waits now report `agent_prompt_stalled` after five seconds without an observed state change instead of waiting indefinitely after an ineffective submission.
- `herdr config check` now reports unknown config keys with their full paths instead of treating ignored typos as valid configuration. (#1573)
- Codex panes with customized static terminal titles now fall back to the live working footer instead of remaining idle, while OSC activity remains preferred. (#1563)
- Grok panes now preserve working and blocked state from terminal signals and pinned background-work status instead of falling back to idle mid-turn.
- OpenCode lifecycle reports are now serialized so out-of-order plugin events cannot leave an idle pane marked working. (#1519)
- Kimi question prompts now report blocked until the user answers or dismisses them.
- Pi lifecycle reporting now uses settled events, preventing transient message boundaries from publishing an idle state mid-turn.
- The Pi, OMP, OpenCode, and Kilo Code integrations can now be installed on Windows and report lifecycle state and native session identity through Herdr's named-pipe API. (#1531)
- Named agent prompts now honor live bracketed-paste mode before sending Enter, preserving OpenCode text such as `A != B` instead of triggering shell mode. (#1525)
- New panes, tabs, layouts, and workspaces using `new_cwd = "follow"` now inherit the foreground process-group leader's working directory instead of an unrelated helper process directory. (#1472)
- Cached pane working directories no longer trigger repeated filesystem checks, avoiding slow sidebar rendering on network filesystems such as Ceph. (#1603)
- Windows foreground-process snapshots are now shared across panes, reducing idle CPU use in sessions with many panes. (#1158)
- Terminal diff streams now batch contiguous writes, reducing the visible wave effect while scrolling pane history. (#283)
- A standalone Escape arriving beside another key is now preserved as its own input instead of being combined into a fabricated Alt chord. (#541)
- Pane viewports that were following live output now continue following after a resize.
- Mouse selections now remain visible when `ui.copy_on_select = false` while clipboard writes stay disabled. (#1471)
- Workspace close confirmation now shows the current workspace name instead of a stale or unrelated label. (#1364)
- Plugin command arrays now preserve whitespace-only arguments. (#1594, #1613)
- Plugins can now be installed or linked while no Herdr server is running. (#1670)
- Remote attach now discovers Herdr installed in mise's canonical tool path before offering to install a sidecar binary. (#1201)
- Noninteractive update, plugin, integration, sound, custom-command, and Git subprocesses no longer flash console windows on Windows. (#1468)
- Live handoff now preserves installed plugins and no longer lets the next plugin installation overwrite the existing registry. (#893)
- `herdr agent wait` now returns `agent_not_running` promptly when its target pane closes instead of waiting for the full timeout. (#1439)
- Pane graphics streams now shut down cleanly when a client disconnect races stream teardown.

## [0.7.4] - 2026-07-15

### Added
- Added session-modal popup floating terminal panes for `type = "popup"` custom command keybindings and plugin panes, with optional cell or percentage sizing and no changes to the tiled tab layout. (#1125)
- Added `ui.copy_on_select` to disable automatic clipboard copying after mouse selection while keeping the selection visible.
- Added configurable row layouts for expanded Space and Agent sidebar entries, including built-in display tokens, per-agent overrides, custom metadata tokens, and pane/workspace metadata reporting through the CLI and socket API.
- Added independent `row_gap` settings for expanded Space and Agent sidebar entries.
- Copy mode now supports literal smart-case search with `/` and `?`, repeating with `n` and `N`, match highlighting, and tmux-style cross-line `w`/`b`/`e` word motions. (#1230)
- Added Maki agent support. (#1301, #1302, thanks @tontinton)
- Added a searchable, version-matched configuration reference and a troubleshooting guide covering duplicate terminal key events, modified-arrow shell bindings, updates, remote access, and logs. (#1116, #1370)

### Changed
- Expanded Space and Agent sidebar entries now use a packed layout by default; set the corresponding `row_gap` to `1` to restore the previous spacing.
- Refreshed the bundled Herdr agent skill for current public workspace, tab, and pane ids and the current CLI/API workflow. (#1297)
- Expanded Japanese and Simplified Chinese CLI documentation with shell completion setup and API schema usage. (#1151)

### Fixed
- Collapsed Agent sidebar rows now follow the same ordering and click targets as the expanded panel, and their shortcut numbers are assigned by visible list position instead of repeating across workspaces. (#1168, #1344)
- Shifted indexed bindings such as `prefix+shift+1..9` now match terminals that report the corresponding punctuation characters. (#1184)
- Plugin-driven tab renames now immediately refresh tab-bar geometry and labels. (#1111, #1179, thanks @kovalov)
- New tabs, splits, layouts, and workspaces configured to follow the foreground directory now start from the focused pane's current working directory. (#1245)
- Amp, Codex, and Claude Code detection now recognizes current active-turn UI variants, including reordered Codex title spinners and Claude `/btw` turns. (#1208, #1281, #1366)
- Pi lifecycle state now reanchors after native session replacement, avoiding working panes that remain idle or tied to an abandoned session. (#943, #1189, thanks @dmmulroy)
- OMP lifecycle reports are now retried when startup races drop the first report. (#1310)
- WSL now uses Herdr's drawn cursor by default, matching the native Windows workaround for host cursor flicker. (#930)
- Live handoff now preserves explicit named-session socket paths, waits for slower server shutdowns, and flushes API responses before the old server exits. (#1180, thanks @dvic)
- The Windows installer no longer rewrites an existing config file or creates a duplicate onboarding line during first-run setup. (#1162)
- Config diagnostics now reach CLI-only and attached-client startup paths reliably and clearly identify fallback configuration behavior.
- Detached custom command children are now reaped after exit instead of accumulating zombie processes. (#1360)
- Renamed single tabs now remain visible in the Agents sidebar instead of losing their tab label. (#1369)
- Documentation search results are now scoped to the active locale and stable or preview channel.
- Horizontal wheel and trackpad events now reach pane applications that enable mouse reporting. (#1349)
- Copy mode `$` and End now stop at the final visible character on the row instead of jumping to the pane edge. (#1405)
- Split SGR mouse reports are now reassembled across input reads, and a preceding standalone Escape is preserved instead of being swallowed or leaked as mouse bytes. (#1334, #1382)
- Linux foreground-process discovery now stays within Herdr pane process trees instead of scanning unrelated host processes, reducing CPU use on busy multi-user systems. (#1399)
- Single-codepoint emoji chosen from the Windows emoji picker now reach panes when WezTerm's kitty keyboard support sends them as CSI-u events with associated text. (#1404)
- Outer-terminal focus gained and lost reports now reach the focused pane when its application enables focus reporting, restoring Neovim file autoreload and other focus-aware terminal behavior. (#1337)
- Native Windows servers now detach from the terminal console that launched them, so closing WezTerm, Windows Terminal, or another host terminal no longer stops persistent pane processes. (#1329)
- Windows API clients now remain connected while waiting for initial named-pipe request bytes, so `status server`, `api snapshot`, and other socket commands no longer intermittently fail with BrokenPipe. (#1279)
- `herdr --remote` now installs remote helper binaries without routing the binary stream through a multiline `/bin/sh -c` command, fixing installs for non-POSIX login shells such as xonsh. (#1203, thanks @nhumrich)
- omp session switches (e.g. resuming a session) no longer crash the agent-state extension on a stale internal variable, which left the pane's agent state stuck.

## [0.7.3] - 2026-07-08

### Fixed
- The session navigator now keeps the active search query when leaving and re-entering search focus, and its footer now shows shortcuts for the current input mode. (#1115, #1140, thanks @liby)
- Re-focusing an already-focused done agent or pane through the socket API now marks it seen instead of leaving stale done status in API responses.
- Windows foreground-process detection now ignores cyclic process-parent snapshots instead of growing memory until the server aborts. (#1083)
- Terminal redraws now hide the cursor inside synchronized output, reducing focused-pane cursor flicker during active redraws. (#967)
- Headless render streams no longer scan visible plain-text URLs during rendering, reducing redraw work while preserving OSC 8 hyperlink metadata.
- The workspace picker once again honors navigate-mode workspace up/down keys, including custom bindings, after `prefix+w`. (#1149)

## [0.7.2] - 2026-07-07

### Added
- Added MastraCode integration support with lifecycle state reports and native thread restore. (#337, #788, thanks @wardpeet)
- Added `ui.sidebar_collapsed_mode = "hidden"` to make a collapsed sidebar use zero width while keeping the existing compact rail as the default. (#842)
- Added `herdr completion <shell>` / `herdr completions <shell>` to generate shell completion scripts for bash, elvish, fish, PowerShell, and zsh. (#435)
- Added `session.snapshot` to bootstrap client runtime state in one socket API response before subscribing to events.
- Added `herdr api schema` to inspect the bundled socket API schema, with `--json` for the full JSON Schema document and `--output PATH` for file output.
- Added `layout.updated` socket events so protocol clients can keep tab layout snapshots current after pane split, resize, swap, move, zoom, and layout mutations.
- Added pane scroll metrics to pane socket API responses and `pane.scroll_changed` subscriptions for clients that need to show when a pane is scrolled back.
- Added `herdr terminal session observe` for read-only live ANSI terminal streams that bridge processes can consume as newline-delimited JSON.
- Added `herdr terminal session control` for bridge processes that need live ANSI frames plus input, resize, scroll, release, and takeover authority.
- Added `ui.hide_tab_bar_when_single_tab` to hide the tab row when a workspace has one tab. (#448)
- Added Japanese and Simplified Chinese website docs.
- Added `bora integration install grok` for Grok CLI (Grok Build) hooks that report session ids through Bora's socket API. Grok state stays screen-detected. When native agent session restore is enabled, Bora can resume Grok panes with `grok --resume <id>`.
- Added `github.pulls.list` and `github.issues.list` socket API methods to read cached open pull requests and issues per repo, plus `github.prs_refreshed` and `github.issues_refreshed` events.
- Create worktree modal opened by a "+" button on each repo header row, with GitHub / Branch / Name tabs. GitHub lists the repo's open pull requests and issues: a pull request opens its worktree, an issue runs the configured `[flow]` command (issue rows are disabled with a hint when no `[flow]` command is set). Branch checks out an existing local branch; Name creates a fresh branch. The existing `new_worktree` keybind and the `GitWorkspace` context-menu "New worktree" entry open the same modal.

### Changed
- The mobile switcher now starts from an agents-first summary and renders worktrees as a tree, making narrow terminals easier to scan.
- macOS prefix input-source switching now runs on the foreground client, so non-Latin input sources are restored reliably after prefix mode. (#774, #1016, thanks @ppggff)
- Nix packaging now uses `xcbuild` instead of custom Apple SDK wrappers for Darwin builds. (#995, thanks @arunoruto)

### Fixed
- Windows clients now send shifted punctuation such as `!`, `?`, and `:` as literal text to Kitty-keyboard-mode pane apps, fixing Kiro CLI TUI prompts while preserving modified key chords. (#1066, #1105)
- Alt-Shift letter chords are now preserved instead of being collapsed into plain uppercase input. (#1088)
- Antigravity background-task waits are now detected even when the UI does not show a `/tasks` hint. (#755)
- `herdr --remote` now prints clean remote attach failures and SSH authentication guidance instead of Rust Debug-formatted I/O errors when SSH authentication is denied. (#1034)
- `herdr server stop` now stops Windows named-pipe servers instead of failing with `named pipes do not support I/O timeouts`. (#1113)
- `herdr server stop` now waits until both server sockets are unreachable before returning, avoiding an immediate first-start failure when restarting right after replacing the binary.
- macOS `herdr --remote` clients now bridge Finder-dropped image files to the remote pane instead of forwarding the local file path as typed text. (#828)
- Grok Build agent detection now tracks the current Grok Build UI: panes report working while responses, tools, and subagents run, and blocked on permission prompts and question dialogs, instead of falling back to idle mid-turn. (#1017, #1055, thanks @TonyxSun)
- GitHub Copilot CLI detection now recognizes the newer Esc interrupt prompt as working. (#1119, #1120, thanks @LaneBirmingham)
- Unix local Herdr clients no longer treat empty bracketed paste as a clipboard-image bridge; `herdr --remote` keeps using it for local-desktop image paste over SSH. (#986)
- Custom command keybindings now run through `cmd.exe /d /c` on Windows instead of `/bin/sh`, so `type = "pane"` and `type = "shell"` bindings can launch native Windows commands. (#1041)
- Plain PageUp/PageDown now reach primary-screen pager apps such as `less -X` and Git diff when they enter application cursor mode, while shell transcripts still use Herdr pane scrollback. (#953)
- Copy mode now supports Ctrl-page navigation, keeps the Herdr prefix key available while copying, and restores the copy context correctly after prefix commands. (#681, #885, #1092, thanks @reobin)
- `prefix+e` scrollback editor panes now open on Windows without trying to run `/bin/sh`; Windows uses `VISUAL`, then `EDITOR`, then `notepad.exe` as the fallback editor. (#914)
- `herdr pane split --current` now resolves to the calling Herdr pane instead of the UI-focused pane when run inside a pane. (#902)
- Native Windows clients running inside Alacritty now preserve mouse reports and `ctrl+j` input instead of leaking mouse escape sequences into panes. `shift+enter` remains dependent on whether the outer terminal reports it as a distinct modified Enter key. (#792)
- Windows clients now preserve bracketed paste, Backspace, modifier-only keys, host cursor drawing, native clipboard copies, recent pane reads, and wait connections across the native input path. (#670, #795, #907, #920, #930, #962, #963, #1067)
- New tabs and workspaces now follow the focused pane's current directory more reliably, including PowerShell panes that report cwd through prompt shell integration on Windows. (#912, #919)
- Pi and OMP integration state now survives internal session reloads, recovers after resumed sessions such as `omp -c`, and reports Ask/tool approval waits as blocked instead of leaving the pane working or stuck on the previous session. (#800, #879, #984, thanks @dmmulroy)
- Pi state socket reports are now retried, reducing stale sidebar state when the report races server startup. (#1049)
- OpenCode now reports subagent permission prompts as blocked and handles object-form `session.status` events. (#838, thanks @soar)
- Remote attach now discovers compatible Homebrew, mise, and Nix profile installs before offering to install a sidecar binary to `~/.local/bin/herdr`. (#840)
- `herdr --remote` sessions now keep the remote server in its own login-independent session and preserve compatible running servers after helper binary updates, so network drops should disconnect only the client instead of killing remote panes.
- `herdr --remote` now reuses one OpenSSH connection across setup probes, installs, server checks, and the final bridge when `[remote].manage_ssh_config` is enabled, so password-based hosts prompt once instead of once per setup command. (#888)
- Foreground agent session reports can now replace stale saved session references, so resumed panes do not stay tied to an older agent session. (#943)
- Kitty graphics panes now repaint streaming image updates reliably and delete replaced host images instead of leaking them. (#947, #948, thanks @DevSrSouza)
- Pane apps that query OSC 12 cursor color now receive a response. (#806)
- ANSI undercurl styles now render in panes. (#895)
- CJK pane border labels, compact keybinding help ranges, and active auto-named tabs now measure by display width, avoiding broken alignment and unreadable labels. (#799, #810, #817, #829)
- Ctrl+/ is now encoded as Ctrl+_, matching terminal expectations for pane apps. (#847)
- PowerShell panes now stay alive after agent Ctrl+C. (#860)
- SGR mouse reports no longer leak into pane input after host-side handling. (#939)
- Wrapped pane links now preserve their target instead of being truncated across soft-wrapped lines. (#1098)
- Linux foreground process-group scans are cached, reducing idle CPU in large sessions. (#936)
- Session autosaves now run off the main loop, reducing UI stalls in busy sessions.
- Worktree removal now focuses the parent workspace after closing the worktree workspace. (#1004)
- Closing a tab from the context menu now exits the menu cleanly. (#945)
- Copy feedback now stays visible above retained pane updates. (#555)
- Windows ARM64 installer fallback now works when the normal checksum path is unavailable. (#897)
- The Create worktree modal's GitHub tab now fetches the repo's open pull requests when the modal opens, instead of relying on the throttled periodic snapshot, so a newly opened pull request appears immediately rather than the modal showing a stale list. Opening a pull request in a worktree now also shows an immediate "opening PR in worktree" toast, giving visible feedback while the worktree is created.
- Opening a pull request whose branch already has a worktree now attaches and focuses that existing worktree instead of failing with a git "already checked out" error. This most often hit pull requests opened by the `[flow]` command, whose worktree already existed at the target path.

## [0.7.1] - 2026-06-24

### Added
- Added `[update].version_check` and `[update].manifest_check` so background Herdr version checks and remote agent-detection manifest checks can be disabled independently. Manual `herdr update` and bundled/local detection manifests still work when the background checks are disabled. (#677)
- Added `HERDR_AGENT=<agent>` as a Linux foreground-process hint for agents hidden behind wrappers such as VMs, Bubblewrap, or `fence`, allowing Herdr to use the named agent's screen manifest when `/proc` cannot expose the real command. (#679)
- Added `ui.pane_borders` and `ui.pane_gaps` to make split pane dividers and spacing configurable. (#271)

### Changed
- Removed the Agents panel workspace/all filter. The panel now always shows all agents, defaults to grouped-by-space ordering, and can switch to priority ordering with `ui.agent_panel_sort = "priority"`. (#318)
- User keybindings now displace conflicting built-in defaults during config load, so overriding a default binding no longer leaves both actions attached to the same key. (#747)
- Worktree creation now checks out an existing local branch when the requested branch already exists instead of failing by trying to create it again. (#729)
- Worktree operations started through the socket API and plugin/UI flows now defer long-running Git work until the app runtime can drive it, keeping clients responsive and preserving plugin lifecycle events for worktree-created panes. (#657, #662, #686)
- OMP, OpenCode, Pi, Devin, and other official hook integrations now scope lifecycle and session reports to the intended root agent process more reliably, reducing stale or cross-process session adoption after restarts, nested commands, and new sessions. (#614, #712, #719, #765)

### Fixed
- Windows Terminal multiline text paste now reaches pane apps as one bracketed paste, so OMP, Pi, and similar prompts no longer submit each pasted line separately. Plain Esc, Shift+Enter, mouse, focus, resize, and Unicode paste handling are preserved on the Windows client path. (#670)
- Local Herdr clients no longer treat raw `Ctrl+V` as a clipboard-image paste trigger, so pane apps such as Vim and Neovim receive block-visual `Ctrl+V` even when the desktop clipboard contains an image. `herdr --remote` keeps `keys.remote_image_paste = "ctrl+v"` by default. (#647)
- Herdr now refreshes cached host terminal colors when terminals report a light/dark color-scheme change, so pane apps that query OSC 10/11 no longer need detach/attach to see updated default colors. Opt-in `[theme].auto_switch` can also switch Herdr's own UI between configured `dark_name` and `light_name` themes. (#675)
- Full-lifecycle hook agents can now recover when an old release/report sequence belongs to a previous agent generation. Herdr keeps process-exit validation active under lifecycle authority and re-anchors hook sequence guards after fresh session references or proven process exits. (#684)
- OMP now reports a native session reference, so an OMP pane reappears in the Agents panel after exiting and rerunning `omp` in the same pane, and Herdr can resume it with `omp --resume=<session>`. Previously the released lifecycle hook stayed suppressed until a server restart. (#614)
- Host terminal color query (OSC 10/11) replies that arrive split at their escape introducer no longer leak as text like `11;rgb:...` into the focused pane, most visible when launching agents that probe terminal colors on startup. (#549)
- Long CJK Git branch names in the sidebar now truncate by display width instead of overflowing or cutting at the wrong cell boundary. (#644)
- Temporary pane commands launched from API flows no longer steal focus from the previously focused pane after they finish. (#658)
- Root agent session restore now ignores child process reports that would otherwise overwrite the saved session for the owning pane. (#712)
- Kitty file-transfer media queries are now answered, allowing pane apps that rely on kitty graphics file support to detect image/file media capability correctly. (#732)
- Idle or slow clients no longer block server writes to other clients while the blocked client is waiting for output. (#726)
- GitHub Copilot CLI `ask_user` accept prompts are now detected as blocked so the Agents panel shows that the pane is waiting for input. (#725)
- Pane reads now skip wide-character spacer cells, avoiding duplicated or malformed output around double-width characters. (#698)
- Split pane border intersections now use the active pane color consistently. (#742, thanks @cullendotdev)
- The Windows installer checksum fallback no longer depends on `Get-FileHash`, improving compatibility with constrained PowerShell environments. (#751)
- Pi launched through npm wrappers on Windows is now detected as Pi instead of a generic wrapped process. (#754)
- Windows builds now force the system ConPTY path through a vendored `portable-pty` patch, avoiding the bundled-path startup failure seen in affected Windows environments. (#761)
- Key release events that fall back to encoded input no longer double-send text into pane apps. (#769)
- Remote clients now allow a longer initial handshake, improving `herdr --remote` startup over high-latency links. (#753)

## [0.7.0] - 2026-06-15

### Added
- Added local plugin v1 support with `plugin.link/list/unlink/enable/disable`, manifest-declared actions, event hooks, managed plugin panes, link handlers, command logs, keybinding integration, and authoring docs under Preview docs.
- Added `herdr plugin install <owner>/<repo>[/subdir...]`, `plugin uninstall`, source metadata in `plugin.list`, offline registry fallback, and a human-readable default `plugin list` with `--json` for scripts.
- Added `herdr plugin config-dir <id>` and automatic plugin config/state directory creation so plugin setup docs can point users at a stable config path.
- Added Devin CLI automatic detection plus `herdr integration install devin` hooks that report session ids for restore with `devin --resume <id>`. Devin state remains screen-detected because Devin hooks do not cover every permission cancellation and user interrupt transition. (#606, #622, thanks @minatoaquaMK2)
- Added supporting plugin host APIs for `pane.current`, `pane.process_info`, `client.window_title.set/clear`, `layout.export/apply`, plugin pane placement, plugin invocation context/env injection, and plugin pane ownership across `pane.move`.
- Added `pane.move` and `herdr pane move` to relocate a running pane into another tab, a new tab, or a new workspace without restarting its terminal process. (#299)
- Tabs containing a zoomed pane are now marked in the tab bar so the zoom state is visible from other tabs.

### Changed
- Bumped the client/server protocol version to 14 for `pane.move` compatibility. (#299)
- Public workspace, tab, and pane ids are now short stable handles such as `w1`, `w1:t1`, and `w1:p1`; closed tab and pane ids no longer retarget later resources. (#569)

### Fixed
- `pane.send_keys` and `pane.send_input.keys` now accept Herdr key-combo strings such as `ctrl+h`, `ctrl+j`, `ctrl+k`, and `ctrl+l`. (#613, thanks @dmmulroy)
- Config startup and reload now warn about unknown top-level table sections, including a `[toast]` hint that points to `[ui.toast]`, instead of silently ignoring them.
- Claude Code session restore now accepts real `/clear`, `/resume`, and compacted session identity changes while still ignoring nested `claude -p` startup sessions that inherit the pane environment. (#620)
- Auto-named tab labels now stay compact after closing, moving, or creating tabs while public tab ids remain stable.
- F1-F4 key presses sent as `ESC[11~` through `ESC[14~` now reach pane apps instead of being dropped. (#574)
- Numeric keypad keys sent through the kitty keyboard protocol now enter their digits and operators instead of being dropped. (#570)
- Pane resize keybindings now shrink panes again instead of only being able to grow them. (#562)
- Windows pane cursor rendering is now stable instead of showing a misplaced or flickering cursor. (#556)
- Tab identity is now preserved across restored sessions.
- Idle panes now poll their PTY less frequently, reducing CPU use while sessions are inactive.
- Captured pane URL clicks, including plugin link handlers, now use Ctrl-click on macOS too because captured terminal mouse reports do not expose Cmd-click separately from plain click. (#307)

## [0.6.10] - 2026-06-11

This is a hotfix release for v0.6.9. See the v0.6.9 notes for the full feature release.

### Fixed
- Lifecycle-authority agent integrations such as Pi and OpenCode no longer trigger a repeated detection reset loop that could flood logs, drive high CPU, and make the UI lag or stop responding. (#560, #565, thanks @dzevs)

## [0.6.9] - 2026-06-10

### Fixed
- Copy mode page scrolling now stops at the same top and bottom boundaries as normal pane scrolling instead of overshooting or getting stuck near the edges. (#459, #460, thanks @reobin)
- Clipboard-copy feedback no longer stays visible after the related selection state has gone stale. (#443)
- The session navigator now uses live workspace labels, so renamed workspaces and cwd-derived labels stay current while navigating. (#377)
- Hermes Agent integration installs now preserve flat plugin-list settings instead of rewriting them into nested lists. (#479)
- Host-terminal focus redraws now stay pending until the client can send them, so panes refresh after focus returns even when redraw delivery was briefly busy.
- Numeric keypad keys that send VT100 application-keypad escape sequences now enter their digits and operators instead of being dropped. (#493)
- Codex panes now stay marked working when the live status header uses reasoning-summary text such as `Investigating code output` instead of the literal `Working` label. (#501)
- Codex blocker detection now ignores stale prompt text outside the live prompt region, reducing false blocked states from old scrollback.
- Native pane URL clicks now use Cmd-click on macOS and Ctrl-click on other platforms. (#307)
- Worktree open, create, and remove actions now work from bare repositories instead of assuming a normal checkout. (#497)
- Pane mouse handling no longer sends empty PTY writes for mouse events that produce no terminal input. (#496)
- Pane output now renders flag emoji and other multi-codepoint grapheme clusters as complete symbols instead of blank cells. (#243)
- Starting Herdr with no restored workspaces, or closing the last workspace, now opens a default workspace instead of leaving the client on an empty screen where direct keybindings such as `cmd+n` were shown but ignored. (#366)
- Resizing restored panes no longer aborts the server when libghostty-vt reflows a terminal whose pre-resize cursor row is past the new height. (#465)
- Full-screen TUIs such as Neovim now receive resize-generated terminal responses after Herdr internal pane resizes, so grown panes redraw without waiting for extra input. (#471)
- Nested agent session reports from child terminals no longer overwrite the owning pane's restored agent session id. (#511)
- Headless servers now avoid repeated scrollback rendering work for inactive panes, reducing CPU in large sessions. (#512)
- Mouse-click handling now respects `ui.prompt_new_tab_name`, so mouse-created tabs follow the same naming prompt setting as keyboard-created tabs. (#521, thanks @imrajyavardhan12)
- Pasting now works in modal text inputs, including rename prompts, command prompts, and worktree dialogs. (#302)
- Linux clipboard image reads now validate image payloads before accepting them, preventing malformed clipboard data from reaching pane image paste flows. (#534)

### Added
- Added remote auto-updates for agent detection manifests, with per-agent validation, local override precedence, `herdr server agent-manifests` diagnostics, and explain output showing remote manifest status.
- Added `herdr server update-agent-manifests` to fetch remote agent detection manifests immediately, reload the running server, and print the updated manifest status.
- Added `herdr agent explain` to show the manifest source, matched rule, evaluated matcher and region evidence, visible evidence flags, skipped-update reason, and idle fallback reason for live panes or saved screen fixtures.
- Added `herdr integration install kimi` for Kimi Code CLI hooks that report lifecycle state and session ids through Herdr's socket API. When native agent session restore is enabled, Herdr can resume Kimi panes with `kimi --session <id>`. (#431, #463, thanks @wbxl2000)
- Added `herdr integration install droid` for Factory Droid hooks that report session ids through Herdr's socket API. When native agent session restore is enabled, Herdr can resume Droid panes with `droid --resume <id>`.
- Added `herdr integration install kilo` for Kilo Code CLI plugins that report lifecycle state and session ids through Herdr's socket API. When native agent session restore is enabled, Herdr can resume Kilo panes with `kilo --session <id>`.
- Added `herdr integration install cursor` for Cursor Agent CLI hooks that report session ids through Herdr's socket API. When native agent session restore is enabled, Herdr can resume Cursor panes with `cursor-agent --resume <id>`. (#506, thanks @udirom)
- Added directional pane swap with `prefix+shift+h/j/k/l`, a pane context-menu swap action, pane layout/neighbor/edge/focus/resize socket APIs, matching CLI commands, and optional `pane split --ratio` support. (#330, #421)
- Added `herdr pane zoom` and the `pane.zoom` socket API to toggle, set, or clear tab-local pane zoom from scripts and integrations.
- Added toast ergonomics controls for delayed agent notifications, in-app toast placement, copied-to-clipboard feedback, and the `notification.show` socket API with `herdr notification show` and optional `none`, `done`, or `request` sounds. (#486)

### Changed
- OpenCode installed with the current Herdr plugin now reports lifecycle state directly instead of relying on screen manifest detection. Kimi Code CLI `0.14.0` or newer now reports full lifecycle state through hooks, including interrupts. Droid and Qoder CLI now report native session identity while leaving lifecycle state to screen manifest detection.

## [0.6.8] - 2026-06-04

This is a hotfix release for v0.6.7, prioritizing a server-crash fix for panes that print complex Unicode or emoji output.

### Fixed
- Fixed a Herdr server crash triggered by pane output containing complex Unicode, emoji, or decomposed accent graphemes. Affected sessions could lose running pane processes or crash again after restore if the same saved pane output was replayed. (#453)
- Direct installs managed by mise now update through the mise install path instead of failing to replace the active binary.
- Claude Code panes that are actively thinking or streaming no longer flicker to blocked because of custom status text. (#409)
- Claude Code panes now detect running shell-command status more reliably.
- OpenCode installed through pnpm is now detected as `opencode` instead of being missed because the packaged executable is named `opencode.exe`. (#447)

### Added
- Added opt-in macOS input-source switching during prefix mode with `experimental.switch_ascii_input_source_in_prefix`, so users typing with a non-Latin IME can run prefix commands through an ASCII-capable input source and return to the previous input source when prefix mode ends. (#400, #434, thanks @sf-jin-ku)

## [0.6.7] - 2026-06-03

### Added
- Added a compact collapse control to the expanded sidebar so mouse users can collapse and expand the sidebar from visible controls. (#278, #291, thanks @turgaybulut)
- Added an opt-in preview update channel with `herdr channel set preview`, `[update].channel`, automated preview manifests, and GitHub prerelease publishing for users who want fixes before stable releases as Herdr transitions toward less frequent, more stable releases.
- Added a remote SSH bridge keepalive fallback. `herdr --remote` now generates a temporary SSH config that includes the user's SSH config first, then adds `ServerAliveInterval` and `ServerAliveCountMax` only when the user has not already configured keepalives. Set `[remote].manage_ssh_config = false` to disable this. (#354, #355, thanks @SunskyXH)
- Added `ui.right_click_passthrough_modifier` so a configured modifier such as `ctrl` can forward right-click hold and drag gestures to mouse-reporting pane apps while normal right-click still opens Herdr's pane menu. (#148)
- Added Kilo Code CLI automatic detection for idle, working, and blocked terminal states. (#270)
- Added `herdr integration install copilot` for GitHub Copilot CLI hooks that report native session ids through Herdr's socket API. Copilot state still comes from Herdr's screen detection because Copilot hooks do not provide complete lifecycle coverage. When native agent session restore is enabled, Herdr can resume Copilot panes with `copilot --resume=<id>`. (#232, #386, thanks @LaneBirmingham)

### Changed
- Native agent session restore is now enabled by default for supported panes with current official integrations. Set `[session] resume_agents_on_restore = false` to disable it.
- Claude Code, Codex, GitHub Copilot CLI, Droid, Kimi Code CLI, and Qoder CLI integrations now report session identity only. Native state for those agents comes from Herdr's screen detection, while Pi, OMP, OpenCode, Kilo Code CLI, Hermes Agent, and custom socket integrations can still report state.

### Fixed
- Large long-running sessions no longer hit the frame-streaming crash fixed by the vendored libghostty-vt update. (#276)
- Copy mode now preserves linewise selection after `shift+v` while moving the cursor. (#360, #389, thanks @reobin)
- Leaving copy mode now restores the previous scroll position, or returns to the bottom when copy mode started at the bottom. (#398, #410, thanks @reobin)
- Git branch labels now resolve correctly in repositories that use Git's reftable ref format instead of showing `.invalid`. (#384, #423, thanks @LaneBirmingham)
- The official Nix flake now builds on macOS by providing Darwin SDK discovery helpers and Darwin cctools to the vendored libghostty-vt build. (#405, #407, thanks @DeevsDeevs)
- Commands launched after `--`, such as `herdr agent start ... -- opencode --session <id>`, now preserve child argv flags instead of parsing them as Herdr flags. (#383)
- Pane apps that request any-motion mouse tracking now receive hover/move events, making Textual-style TUI mouse interaction more reliable inside Herdr. (#419)
- Claude Code background-agent wait text in scrollback no longer keeps an idle pane marked working after the background agent has completed.
- Claude Code and Codex transcript or expanded-detail viewers no longer publish a false idle state while the pane is still showing active agent status.
- Claude Code question prompts that use the arrow-glyph selector are now detected as blocked.
- Kiro sub-agent tool approval prompts are now detected as blocked instead of working. (#388)
- Shift-letter prefix bindings such as `prefix+shift+n` now work in legacy SSH terminal sessions that send uppercase letters without separate Shift metadata. (#312)
- Idle panes now avoid repeated full foreground-process scans, reducing idle CPU on sessions with many panes. (#439)
- Restored native agent sessions now resume across background workspaces and tabs after the first client provides terminal context instead of waiting until each pane is focused.
- Pane input no longer waits behind the PTY actor's idle read poll, restoring responsive typing at quiet shell prompts. (#379)
- Pane apps that query OSC 4 ANSI palette colors now receive the active terminal palette response, so OpenCode and similar TUIs can enable system-theme behavior inside Herdr. (#387)
- Pane apps that query terminal capabilities with XTGETTCAP now receive supported capability responses, improving feature detection in Neovim and similar terminal apps. (#393)
- Pane text selection now derives its highlight colors from the host terminal or active Herdr palette instead of forcing the theme's blue accent. (#298)
- `herdr channel set preview` and `herdr channel set stable` now update direct installs from the selected channel immediately, reject preview on Homebrew and Nix installs before changing config, and show package-manager guidance for managed installs.
- Plain `herdr update` and remote binary replacement now ask before stopping running sessions, avoid protocol-heavy prompt text, and leave the current install untouched when the user chooses not to stop active pane processes. Explicit `--handoff` update flows try live handoff without a second handoff prompt.
- Remote bootstrap now uses the remote shell only for PATH discovery and runs internal probes through `/bin/sh`, so `herdr --remote` can detect existing installs when the remote login shell is fish. (#396)

## [0.6.6] - 2026-05-31

### Added
- Custom command keybindings now accept an optional `description` field to provide user-defined descriptions shown in the keybind help panel instead of the default `'custom command'` label. (#362)

### Fixed
- The OpenCode integration no longer treats `session.created` or `session.updated` plugin events as idle signals, so active sessions stay marked working until OpenCode reports `session.status` or `session.idle`. (#351)
- New interactive panes now use login-shell startup on macOS by default so Homebrew and other login PATH setup is available, with `terminal.shell_mode = "non_login"` as an opt-out. (#350)
- Claude Code panes no longer stay blocked after stale permission-prompt reports when the visible screen has returned to idle or working state. (#349)
- Codex panes no longer stay working because stale `esc to interrupt` text remains above a visible idle prompt, and visible approval-review work is now preserved as working. (#352)
- Sidebar Git status refresh now deduplicates workspaces from the same checkout and reuses cached ahead/behind results when refs have not changed, reducing idle CPU from repeated `git` polling. (#353)
- Update prompts, toasts, and docs now distinguish installing a new binary from stopping or reattaching a running Herdr session to use it.
- Large restored sessions no longer leave restored or newly split panes without shells after startup, and live handoff keeps PTY ownership bounded to one master fd per pane. (#357)
- Pane shutdown no longer warns that a pane is still alive after the direct child has already exited and been reaped. (#338)
- Closing the last pane or tab in a parent worktree workspace now shows the existing confirmation before closing the whole worktree group. (#369)

## [0.6.5] - 2026-05-29

### Added
- Added pane copy mode at `prefix+[` with keyboard navigation, visual selection, and clipboard yank support. (#231)
- Added `foreground_cwd` to pane and agent API/CLI responses so integrations can inspect the active foreground process directory without changing the existing pane/workspace `cwd` semantics. (#345)
- Added read-only `agent_session` metadata to pane and agent API/CLI responses when official integrations report native session references.

### Fixed
- Live handoff now preserves terminal state when transferring supported running panes to a replacement server.
- WSL clipboard writes now prefer OSC 52 before WSLg clipboard tools, so mouse selection and double-click copy populate Windows clipboard history in Windows Terminal. (#333)
- Incomplete host terminal OSC default-color replies no longer get misread as Alt-key input and forwarded into panes, preventing interactive prompts such as `gh auth login --web` from aborting on split `ESC ]` input. (#279, #306, #344)
- Workspace rename prompts and background notifications now use live cwd-derived workspace labels instead of stale session labels. (#332)
- `herdr session stop` no longer fails on zero-duration socket timeouts when the stop deadline is nearly exhausted.
- Update preview instructions now wrap long package-manager commands instead of truncating the shell command suffix.
- Restored native agent resume panes now fall back to a shell when the resumed agent exits instead of closing the whole pane.

## [0.6.4] - 2026-05-27

### Fixed
- Fixed macOS server startup with large restored sessions by raising the server file descriptor soft limit, preventing new panes from failing with `dup of fd N failed` or `Too many open files` around 40 live panes. (#327)

This is a hotfix for v0.6.3. See the v0.6.3 notes for the full feature release.

## [0.6.3] - 2026-05-27

### Added
- Added native agent session restore behind `[session] resume_agents_on_restore`, allowing supported Pi, Claude Code, Codex, OpenCode, and Hermes panes with current official integrations to restart into their previous agent conversation after a Herdr server restart. (#233)
- Added opt-in pane screen history across full server restarts with `[experimental] pane_history = true` and Settings > Experiments > pane screen history. (#217, #248, thanks @icedac)
- Added a session navigator at `prefix+g` with a searchable workspace/tab/pane tree, agent state filters, mouse switching, and keyboard navigation. (#157)
- Added configurable navigate-mode movement bindings for workspace and pane navigation keys. (#193)
- Added a configurable `last_pane` keybinding action for tmux-style back-and-forth navigation to the last focused pane across workspaces and tabs. It is unset by default. (#287)
- Added scrollback support to direct agent terminal attaches. Mouse wheel and plain PageUp/PageDown now scroll the attached terminal viewport, while terminal apps that request mouse or alternate-scroll input still receive those events. The client/server protocol is now version 11.
- Added `ui.redraw_on_focus_gained` to keep the existing full redraw on outer-terminal focus gain by default while allowing users to opt out of the visible refresh. (#282)
- Added `ui.mobile_width_threshold` to configure the terminal width at which Herdr switches to the mobile single-column layout. (#317)
- Added `--handoff` for `herdr update` and `herdr --remote` to opt into live server handoff for supported running servers. Plain update and remote attach use the normal restart/stop flow by default.
- Added `pane.report_metadata` and `herdr pane report-metadata` so user hooks can customize pane titles, displayed agent names, compact status labels, and visible state labels without taking over integration-owned lifecycle or session state. (#36)
- Added tmux-style double-click token copy in panes, with temporary copy feedback and mouse passthrough preserved for terminal apps that request mouse input. (#142, #296, thanks @babymastodon)
- Added Ctrl-click URL opening inside panes for OSC 8 hyperlinks and visible `http://` or `https://` URLs when the host terminal sends the modified click to Herdr. (#307)
- Added Qoder CLI detection, terminal state heuristics, and `herdr integration install qodercli` hook support. (#308, #309, thanks @wayneleelwc)

### Fixed
- Remote bootstrap now downloads exact-version release assets for Homebrew and Nix clients instead of copying package-manager-managed local binaries into `~/.local/bin/herdr`.
- `website/latest.json` now stores asset URLs for archived releases under `releases[version].assets`, so remote bootstrap can fetch the current client version even when Homebrew and the top-level latest release are temporarily out of sync.
- App and server event queues no longer stall under load, improving delivery of pane and agent state updates. (#265)
- Agent status subscriptions now deliver already-matching states and event-hub notifications reliably for waits and automation. (#288, #295)
- Codex background terminal waits are detected more reliably, and idle agent checking uses less CPU. (#300)
- Split OSC 10/11 host color replies are buffered correctly, so terminal apps still receive host foreground/background color responses when replies arrive in chunks. (#306, #310)
- `herdr session stop` is more reliable when the server closes the socket early or stops without sending a full response.
- The OpenCode integration now releases pane ownership on plugin dispose, preventing stale integration state after OpenCode exits. (#314)
- Linux sound alerts no longer fall back to `aplay` for mp3 files, preventing static noise on systems without `paplay`. Herdr now tries mp3-capable players such as `pw-play`, `ffplay`, `mpg123`, and `mpv` instead. (#290)

## [0.6.2] - 2026-05-23

### Added
- Added optional Nix flake support for building, running, installing, and developing Herdr with Nix. (#208, #221, #264)
- Added `terminal.new_cwd` to choose whether new panes, tabs, and workspaces follow the source pane/workspace, start in `$HOME`, use Herdr's process directory, or use a fixed path.
- Added `herdr integration install omp` for OMP's `.omp` extension directory. The extension reports OMP pane state through Herdr's socket API without relying on native `omp` process detection.
- Added CLI and socket API support for Git worktrees with `herdr worktree list/create/open/remove`, optional worktree provenance on workspace responses, and client/server protocol version 10.

### Fixed
- GitHub Copilot CLI sessions now use tested terminal heuristics for approval prompts, freeform input, plan review, and thinking states in the Agents panel. (#232, #256, thanks @LaneBirmingham)
- Kiro approval prompts are now detected as blocked in the Agents panel. (#255)
- Workspace labels now follow the live pane working directory after directory changes.
- Remote clients using local keybindings no longer show stale server keybinding warnings from the remote host.

## [0.6.1] - 2026-05-22

### Added
- Added `ui.mouse_scroll_lines` to configure how many pane scrollback lines each mouse wheel notch scrolls. The default remains 3. (#236)
- Added `--remote-keybindings local|server` for `herdr --remote`. Remote attach now uses the launching client's local keybindings by default without copying config files to the remote host; use `--remote-keybindings server` to keep the remote server's keybindings. The client/server protocol is now version 9.
- Added `experimental.reveal_hidden_cursor_for_cjk_ime = false` (opt-in), `experimental.cjk_ime_agents = []` (optional allow-list), and `experimental.cjk_ime_cursor_shape = "steady_block"` to expose the focused pane's cursor anchor to the outer terminal even when the pane requested `?25l`, restoring macOS IME candidate-window tracking for TUIs that paint their own cursor (Claude Code, pi, codex). When `cjk_ime_agents` is non-empty, the reveal applies only to focused panes whose detected agent matches one of the listed names. When the pane reports no cursor position, the anchor falls back to the pane's top-left so a stable IME hint is always available. Trade-off when enabled: an extra hardware cursor may appear in the outer terminal for apps that hide the cursor without painting a replacement. (#149, thanks @ChihGodlee)
- Added explicit sidebar Git worktree groups plus native worktree creation, existing checkout open, and safe checkout cleanup flows, configured by `[worktrees].directory`, `keys.new_worktree`, optional `keys.open_worktree`, and optional `keys.remove_worktree`. (#137)
- Added named-session reattach and stop command hints so detach and update guidance point back to the active session. (#199, thanks @Golden-Pigeon)

### Fixed
- Pane apps that query OSC 10/11 default foreground/background colors now receive the host terminal colors, so OpenCode and similar TUIs can detect light terminal themes inside Herdr. (#253)
- Codex Plan mode question prompts now override stale integration `working` reports when the visible terminal UI is clearly waiting for an answer, stale hook authority is cleared when foreground process detection sees Codex exit back to the shell, and Claude Code cancellations now recover from stale hook `working` reports when the idle prompt returns. (#249)
- Keybinding parsing now accepts non-ASCII printable keys such as `ö`, `é`, and `ğ`, including UTF-8 Alt chords. (#247)
- Kimi Code CLI sessions now use structural terminal detection for approval prompts and live thinking/tool status, improving working and blocked state reporting in the Agents panel. (#215)
- Antigravity CLI (`agy`) sessions are now detected, and their terminal UI now reports working and blocked states in the Agents panel. (#207)
- Cursor Agent sessions launched as `cursor-agent` or symlink aliases such as `agent` are now detected, and their terminal UI now reports working and blocked states in the Agents panel. (#225)
- Agent detection now ignores runtime argument strings when identifying foreground processes, reducing false positives from helper commands and wrapped processes. (#238)
- In-app notifications now stay below interactive floating overlays, so dialogs and menus remain readable and clickable while a toast is visible. (#228)
- `herdr --remote` now offers to restart the remote server after installing or replacing a remote binary, or when the running server version differs, even if the client/server protocol is still compatible.

## [0.6.0] - 2026-05-20

### Added
- Added keybinding v2 with explicit `prefix+...` syntax, array bindings per action, configurable prefix-mode pane focus, tab switching, and direct modified chords for users who opt in. (#154, #201, #202, #219)
- Added `herdr config reset-keys` to back up `config.toml` and remove custom keybindings so built-in v2 defaults apply on restart or config reload. (#154)
- Added an integrations tab in settings and first-run onboarding so users can install recommended agent integrations from inside Herdr.
- Added update badges on the sidebar menu, settings menu item, and integrations settings tab when installed integrations are outdated.
- Added `terminal.default_shell` to choose the executable used for new interactive panes. When unset, Herdr still falls back to `$SHELL`, then `/bin/sh`. (#196)
- Added native Kiro CLI detection with idle and working state heuristics. (#185)

### Fixed
- Keybinding conflict warnings now stay visible and show one readable yellow row per conflicting binding.
- Update prompts that need to stop a running server now default Enter to yes and show `[Y/n]`.
- Pending release notes no longer open automatically on startup; the latest notes remain available from the menu.
- Running `herdr server` directly now prints socket and log paths and explains that normal TUI users should run `herdr`.
- Kitty graphics virtual Unicode placeholders now render image placements instead of leaving placeholder cells behind. (#136)
- Clipboard image reads are now capped to Herdr's image payload limit, preventing oversized local clipboard images from being read into memory.
- The install script now reads Herdr's public latest-release manifest, so fresh installs use the same binary URLs as `herdr update`.
- The Claude Code integration no longer lets subagent completion hooks report durable `working`, preventing delayed recap or subagent completion events from reviving an idle pane. (#198)
- Remote clients now bridge local clipboard images into the remote pane by staging them as temporary image files and pasting the remote path, so Claude Code image paste works over `herdr --remote`. (#205)

### Breaking Changes
- Removed the separate `keys.quit` binding. Use `keys.detach`, which detaches in server mode and exits in `--no-session` mode. The default detach binding is now `prefix+q`.
- Keybindings now use explicit trigger syntax: `prefix+c` means prefix mode, while `ctrl+alt+c` is direct. Bare printable direct bindings such as `new_tab = "c"` are rejected with diagnostics because they intercept normal typing. The default keymap now gives tmux-style tab actions to `prefix+c`, `prefix+n`/`prefix+p`, and `prefix+1..9`, uses `prefix+w` for workspace navigation, and moves pane focus to `prefix+h/j/k/l`. (#154)
- The client/server protocol is now version 8. Stop and restart any running v0.5.12 server before attaching with this release.

## [0.5.12] - 2026-05-19

### Fixed
- The Claude Code integration no longer reports successful or failed post-tool hooks as `working`, and installing the updated integration removes Herdr's deprecated post-tool hook entries from existing Claude settings. (#198)
- The Codex integration now reports native `PermissionRequest` hooks as `blocked`, so permission prompts no longer stay pinned as `working` after a tool-use hook. (#198)
- Workspace and tab rename prompts now handle Backspace, Ctrl+Backspace, Alt+Backspace, Cmd+Backspace, Ctrl+H, Ctrl+W, and Ctrl+U as editing shortcuts instead of inserting stray characters or clearing unexpectedly. (#204)

## [0.5.11] - 2026-05-19

### Added
- Added the `terminal` built-in theme, which uses the host terminal's ANSI palette for Herdr UI colors. (#140, #146, thanks @babymastodon)
- Added Hermes Agent foreground-process detection with basic idle, working, and blocked heuristics. (#144)
- Added a Hermes Agent plugin integration for direct state reporting. (#144)
- Added `ui.sidebar_min_width` and `ui.sidebar_max_width` to configure the sidebar's expanded resize bounds. Defaults remain 18 and 36 columns; existing configs are unchanged. (#132, #135, thanks @ChihGodlee)

### Fixed
- Running the internal `herdr client` command from inside Herdr now respects the nested-launch guard, and the command is no longer advertised in root help. (#187)
- The Herdr agent skill now refuses to claim pane ownership unless it is running inside Herdr. (#152)
- Terminal-style docs code blocks now keep their copy button in the top-right corner. (#190)
- The sidebar `new` workspace button now aligns with the sidebar's left padding. (#189)
- Herdr now preserves `session.json` symlinks when saving persistent session state. (#139, #147, thanks @cloudmanic)
- Alt+Backspace is now preserved when forwarded into panes. (#155, #165)
- Directional pane focus now works while a tab is zoomed. (#151, #167)
- Agent detection now prefers the foreground process group leader, reducing false matches from child helper processes. (#161, #172)
- Remote attach now uses a matching `herdr` already available on the remote `PATH` before installing a new copy. (#170)
- Modified Enter input such as Shift+Enter is now preserved in supported terminals. (#168)
- Sidebar agent entries now show user-assigned agent names when available. (#145)

### Breaking Changes
- The client/server protocol is now version 7. Stop and restart any running v0.5.10 server before attaching with this release.

## [0.5.10] - 2026-05-17

### Added
- Added indexed keybind families under `[keys.indexed]` for jumping directly to workspace, tab, or visible agent positions 1-9.
- Added hook-owned custom agent status labels, so integrations can show short visual states like `indexing` without changing semantic agent status.
- Added terminal-backed agent commands and socket API methods for listing, reading, sending to, renaming, focusing, waiting on, attaching to, and starting agent terminals.
- Added direct terminal attach with `herdr agent attach <target>` and `herdr terminal attach <terminal_id>`.
- Added `ui.prompt_new_tab_name = false` for creating new tabs immediately with generated names instead of opening the rename dialog. (#123)
- Added optional `keys.edit_scrollback` to open the focused pane's retained scrollback in `$EDITOR` inside a temporary zoomed pane. (#122)

### Changed
- Renamed the focused pane fullscreen keybinding to `keys.zoom`; `keys.fullscreen` remains supported as a legacy alias.

### Fixed
- Grok Build is now detected as `grok`, with basic working, blocked, and idle state detection. Conflicting known-agent hook labels are ignored once native foreground-process detection identifies a different known agent. (#133)
- Terminal cursor shapes now forward through attached clients. (#116)
- Herdr now redraws immediately when the outer terminal regains focus.
- GitHub Copilot is now correctly detected when its process name is `copilot`. (#118)
- Integration installs now respect `PI_CODING_AGENT_DIR`, `CLAUDE_CONFIG_DIR`, and `CODEX_HOME` when choosing Pi, Claude Code, and Codex config paths. (#121)
- Split pane resize hit areas no longer overlap the first content column or row, making text selection work from the start of right and bottom panes. (#120)
- Dragging text selections near pane edges now autoscrolls into scrollback, and selection state now clears correctly when switching workspaces, tabs, or panes. (#128, #129, thanks @leeeanh)
- Zoomed panes now keep their border visible in tabs that contain multiple panes. (#115)

## [0.5.9] - 2026-05-15

### Added
- Added experimental Kitty graphics rendering for local panes and attached clients behind `experimental.kitty_graphics`, including support for larger graphics frames.
- Added `ui.toast.delivery = "system"` for OS-level background notifications, using `notify-send` on Linux and `terminal-notifier` or `osascript` on macOS.
- Added light variants for Catppuccin, Tokyo Night, Gruvbox, One, Solarized, Kanagawa, and Rosé Pine themes.
- Added `ui.mouse_capture = false` for tmux-style mouse behavior, letting the terminal handle normal clicks while still forwarding mouse input to pane apps that request it.

### Changed
- Moved experimental settings into `[experimental]`.

### Fixed
- PageUp and PageDown now scroll Herdr pane scrollback for normal panes while still forwarding keys to full-screen or mouse-reporting apps.
- Enhanced tilde key sequences now parse correctly, improving compatibility with terminals that emit them.
- `herdr integration install codex` now enables the current Codex `[features] hooks = true` flag and migrates the deprecated top-level `codex_hooks` flag.

### Breaking Changes
- `advanced.allow_nested` has moved to `experimental.allow_nested`; update configs that allow nested Herdr launches.
- The client/server protocol is now version 5. Stop and restart any running v0.5.8 server before attaching with this release.

## [0.5.8] - 2026-05-12

### Added
- Added manual pane labels through `herdr pane rename`, the `pane.rename` socket API, an optional `keys.rename_pane` binding, and the right-click pane menu.
- Added `ui.show_agent_labels_on_pane_borders`, which can show detected or reported agent names in split pane borders when no manual pane label is set.
- Added `herdr integration status [--outdated-only]` so installed agent integrations can be checked for legacy or outdated versions.
- Added an optional `keys.open_notification_target` binding for jumping to the pane behind the current notification.
- Added optional `keys.previous_agent` and `keys.next_agent` bindings for cycling through sidebar agent entries.

### Changed
- Scrolling over the tab bar now switches tabs directly, including overflowing tab bars.

### Fixed
- Indexed terminal palette colors now render correctly for 256-color terminal apps.
- Hook-based agent integrations now reject stale out-of-order reports and base notifications on effective agent state, reducing duplicate or stuck state changes.
- Background tabs now resize when the outer terminal size changes, preventing stale pane dimensions when switching back to them.
- Client shutdown now drains queued control messages more reliably.
- Pane cursors are now hidden while scrolled back, and omitted while the mobile switcher is open.
- Mobile agent switcher entries now include tab context, making agents easier to identify on narrow terminals.
- macOS foreground job detection now uses process groups, improving agent state tracking for foreground commands.
- Remote SSH no longer fails before connecting when macOS temporary bridge socket paths exceed Unix socket length limits. (#103, thanks @moonsphere)
- Nix-wrapped agent commands are now detected by their underlying agent entrypoint.
- Pane renames made through the socket API now rerender immediately.

## [0.5.7] - 2026-05-10

### Added
- Added ANSI-formatted pane reads to the CLI and socket API with `herdr pane read --format ansi` / `--ansi`, preserving colors and styles for visible and recent pane output.

### Changed
- The agents panel now highlights the currently focused agent entry, matching the active workspace styling. (#84, thanks @soomtong)

### Fixed
- Git branch and ahead/behind refreshes now run off the main loop, preventing slow Git status checks from freezing the UI.
- Update and startup flows now detect incompatible running servers earlier and give clear stop/restart guidance instead of trying to attach with a mismatched client/server protocol.
- `herdr update` now downloads and prepares the new binary before stopping a running server, reducing the chance of interrupting an active session when download or install preparation fails.

## [0.5.6] - 2026-05-09

### Added
- Added the `vesper` built-in theme. (#71, thanks @nexxeln)
- Added `herdr --remote <ssh-target>`, so you can use Herdr as a thin client for remote servers without SSHing in first. Herdr connects over SSH, bootstraps a matching remote `herdr` binary when needed, starts the remote server automatically, and streams an efficient terminal view back to your local terminal.

### Changed
- Updated the bundled `libghostty-vt` engine and removed the custom Linux C++ runtime link workaround from static builds.
- CLI workspace, tab, and pane creation now preserve the current focus by default; pass `--focus` to switch to the newly created item.

### Fixed
- OSC 8 hyperlinks emitted inside panes now remain clickable after Herdr renders them, including titled markdown-style links.
- Agent panel scope now defaults to `all` and is saved to config when changed, so choosing `current` or `all` survives session resets and upgrades.
- Native agent hook state now clears when the detected native agent exits, preventing stale hook-reported status from sticking to a pane.
- Clicking an in-app agent toast now jumps to the relevant pane and clears the toast after focus.

## [0.5.5] - 2026-05-06

### Added
- Added a mobile layout for narrow terminals, making it practical to SSH into your machine and run herdr from your phone.

### Fixed
- Non-ASCII terminal input is no longer dropped when UTF-8 characters arrive split across multiple reads.
- Native agent detection now clears agents after their foreground process exits and control returns to the shell, preventing stale agent status in the sidebar.
- Pane contents no longer shift horizontally when scrollback appears, keeping the scrollbar gutter stable.

## [0.5.4] - 2026-05-03

### Fixed
- Visible active-tab panes that finish while the outer terminal is unfocused are now marked as seen when you return to herdr, preventing stale done/attention indicators.
- IME candidate windows and mobile SSH cursor tracking now stay anchored to the focused pane during client redraws, including apps that hide the cursor, instead of drifting to sidebar or repaint positions.

## [0.5.3] - 2026-04-30

### Added
- Added named persistent sessions, so you can keep separate herdr environments for different projects or contexts while sharing the same global config. See the docs for the full session CLI. (#57, thanks @fbettag)
- Added `herdr status`, `herdr status server`, and `herdr status client` to inspect the local client, running server, protocol compatibility, socket path, and whether a restart is needed.

### Changed
- Focused panes can now still alert you through terminal notifications when the herdr terminal window is unfocused, so active work does not go quiet just because you switched to another app.

### Fixed
- Dragging pane split borders now works when the app inside the pane has mouse reporting enabled, including Claude Code no-flicker mode. (#61, thanks @EYH0602)
- Pressing the prefix key twice now forwards a literal prefix key into the focused pane in client mode again.
- `herdr integration install` and `herdr integration uninstall` now work without requiring a running herdr server.
- Pane PTYs now keep their last attached size while detached, preventing detached output from being resized or rewrapped to fallback dimensions.

## [0.5.2] - 2026-04-27

### Added
- Config can now be reloaded in the running app/server from the global menu or with `herdr server reload-config`, applying safe live settings without restarting the persistent server.

### Fixed
- Persistent server startup now surfaces config diagnostics in attached clients instead of silently hiding parse or validation errors.
- Pane backgrounds now stay transparent when the host terminal background color is unknown, while explicit terminal cell backgrounds still render correctly.
- Persistent-session toast and sound notifications now target the foreground attached client instead of firing across every connected client.
- Claude Code subagent hook events no longer make the parent Claude pane look idle or released when a subagent finishes, and permissioned tool-call completion keeps the pane in the correct working state.

## [0.5.1] - 2026-04-25

### Added
- Toast notifications can now be delivered through the outer terminal as desktop notifications. Configure this with `ui.toast.delivery = "terminal"`; see the [configuration docs](https://herdr.dev/docs/configuration/) for details.
- Herdr now writes separate capped support logs for app, client, and server modes, making persistent-session issue reports easier to diagnose without unbounded log growth.
- The bundled opencode plugin now reports question prompts as blocked while waiting for user input, then returns to working or idle when answered or dismissed. Question prompts are also detected by the default terminal-screen heuristics. (#51, thanks @mspiegel31)

### Changed
- Routine API request traces now log at debug level by default, making normal support logs smaller and easier to read while preserving detailed traces when debug logging is enabled.

### Fixed
- Pasted text and other reverse-video terminal content now stays readable when pane backgrounds are transparent. (#45, thanks @EYH0602)
- Panes now advertise a stable `TERM=xterm-256color` and `COLORTERM=truecolor` by default, improving redraw and cursor behavior in shells and remote sessions.
- Pane scrollbars once again reserve their own rightmost column instead of overlaying terminal content in persistent session mode.
- Terminal-delivered toast notifications now use the server-approved delivery decision in persistent session mode, so attaching clients do not incorrectly suppress them.
- In-app toast delivery now stays inside herdr instead of also forwarding a terminal/desktop notification.

## [0.5.0] - 2026-04-21

### Breaking Changes Please Read
- herdr now defaults to a persistent server/client session model. running `herdr` starts or reattaches to a background session server instead of launching the old single-process UI.
- quitting the UI in default mode now detaches the current client and leaves the shared session running. use `herdr server stop` to stop the background server explicitly.
- the old monolithic behavior is still available as an escape hatch with `herdr --no-session`.

### Added
- Persistent sessions are now the default product behavior. You can detach and reattach without stopping pane processes.
- Added the thin client and headless server as first-class product components, including auto-detect launch, explicit `herdr client`, and `herdr server stop`.
- Sessions now restore cleanly after full restart, preserving workspaces, tabs, panes, and running process state.
- Multi-client attach is now supported. Multiple clients can connect to the same shared session.

### Changed
- In persistence mode, in-app quit actions now detach the current client by default instead of shutting down the whole background server.
- The current persistence model is a shared session view across attached clients. It is not yet full tmux-style per-client independent navigation.
- Restored sessions now land in terminal mode, while fresh sessions still start in navigate mode.

## [0.4.11] - 2026-04-16

### Breaking Changes Please Read
- The update flow changes in `0.4.11`. Herdr no longer installs updates silently in the background. Starting with this release, herdr only checks for updates and shows them in the UI. To install a new release, quit herdr and then run `herdr update` manually in your shell.
- This prepares the upcoming `0.5.0` persistence release. Herdr is moving from the old single-binary update model toward a persistent server/client session model, so your workspace can keep running while clients attach, detach, and reconnect.
- The reason for this change is upgrade safety. Herdr needs to stop the old running process cleanly before the new client/server model takes over, so manual update avoids mixed-version states during the transition.

### Added
- Hook-reported agent state can now use custom agent labels, so integrations are no longer limited to herdr’s built-in agent names. Custom labels now flow through pane/workspace UI and the socket API anywhere agent names are shown.

## [0.4.10] - 2026-04-14

### Added
- Prefix mode now supports custom command keybindings via `[[keys.command]]`, so you can launch detached shell helpers or open temporary overlay panes from inside herdr using the active workspace, tab, pane, and cwd context.
- Pressing the prefix key twice now forwards a literal prefix keystroke into the focused pane, which makes nested tools and terminal apps that use the same prefix easier to control.

### Fixed
- App-level key handling now normalizes enhanced keyboard reporting consistently, so shifted bindings and text like `?` and uppercase characters work correctly in navigate mode and text-entry UI.
- Ctrl+letter input is now encoded correctly when pane apps enable kitty keyboard mode, improving compatibility with terminal programs that expect CSI-u style key reporting.
- The collapsed sidebar now keeps the active workspace visibly highlighted even while you stay in terminal mode.
- Droid Mission Control screens are now treated as idle instead of active work, reducing false busy-state detection.

## [0.4.9] - 2026-04-13

### Fixed
- Droid's primary-screen redraws no longer erase pane scrollback inside herdr, while normal scrollback-clear behavior is preserved elsewhere.
- `q` is now dedicated to quitting in navigate mode instead of also acting as a generic cancel key in modals and overlays, reducing accidental quits.
- Tab bar scrolling is tighter: the scroll-right button and new-tab button now sit directly adjacent to the last visible tab without a gap, and manual scroll no longer overscrolls past the last tab.

## [0.4.8] - 2026-04-12

### Added
- Themes can now set `panel_bg = "reset"` to let herdr’s panel chrome inherit the host terminal background instead of painting an opaque panel fill. This also accepts the aliases `default`, `none`, and `transparent`.
- Ghostty-backed panes now preserve the host terminal’s default background when it matches the outer terminal theme, so terminal window transparency can show through pane content instead of being repainted as an opaque color.

### Fixed
- Clipboard writes now prefer native platform clipboard tools (`pbcopy`, `wl-copy`, `xclip`, or `xsel`) before falling back to OSC 52, which makes copy operations from panes more reliable across terminal setups.

## [0.4.7] - 2026-04-10

### Added
- The tab bar now handles large tab sets better: you can scroll overflowing tabs with the mouse controls or wheel, and reorder tabs by dragging them.
- `workspace create` and `tab create` now return the created root pane in their JSON response, so automation can act on the new pane immediately without an extra lookup.

### Fixed
- Background panes that start idle no longer show up as `done` or trigger finished-state attention until they have actually transitioned from working or blocked to idle.
- Left-click now focuses panes and right-click now opens the pane context menu even when the inner TUI has mouse reporting enabled, fixing apps like Claude Code. (#25, thanks @othavioquiliao)
- OSC 52 clipboard writes from apps running inside panes now reach the host clipboard correctly, including copy requests emitted by child processes inside the pane.
- `pane close` now removes only the targeted tab when other tabs still exist in the workspace, instead of closing the whole workspace.
- Amp approval prompts are now detected more reliably as blocked, including tool-call, command, and file edit/create approval screens.

### Breaking Changes
- Socket API clients that match `result.type` exactly need to handle `workspace_created` and `tab_created` for `workspace.create` and `tab.create`; these calls no longer return `workspace_info` and `tab_info`.

## [0.4.6] - 2026-04-09

### Fixed
- Agent state detection is now more reliable when panes are scrolled back, when Codex is running in narrow panes, and when Claude opens slash-command or settings menus, reducing false blocked or idle states.
- Mouse-driven terminal text selection now autoscrolls into pane scrollback and clears cleanly after copy, so selecting beyond the visible viewport works as expected.
- Pane terminal colors now return to the outer terminal theme after fullscreen TUIs exit, fixing cases like Droid leaving stale background colors behind. This restore path now also works correctly on macOS.

## [0.4.5] - 2026-04-09

### Added
- `herdr workspace create` and `herdr tab create` now support `--label`, so scripts and agents can name new workspaces and tabs immediately instead of creating them first and renaming them afterward.
- The global menu now includes a manual **reload keybinds** action, so you can apply `config.toml` keybinding changes without restarting herdr.
- The socket API and CLI now expose a `done` agent status, including `herdr wait agent-status --status done`, so automation can distinguish finished agent runs from panes that are merely idle.

### Changed
- Session state is now saved automatically with a debounce while you work, so recent workspace, tab, pane, and sidebar changes are preserved more reliably even if herdr exits unexpectedly.

### Fixed
- Only the focused pane now owns the terminal cursor, which removes stray cursor blocks from unfocused panes.
- In-app **What's New** / release notes now render inline code spans and fenced code blocks correctly.
- Default numbered tabs now stay auto-named when you keep or rename them back to their numeric label, so generated tab numbering stays compact and predictable.

## [0.4.4] - 2026-04-08

### Changed
- The expanded sidebar can now be split into resizable workspace and agent sections with a draggable divider, and that section sizing is preserved across restarts.

### Fixed
- IME input now works properly for Chinese and other UTF-8 input methods in pane terminals, so candidate selection no longer falls back to typing raw digit keys. (#9, thanks @Edmund-a7)
- `herdr pane run ...` now uses the bracketed-paste-aware input path, improving compatibility with shells and terminal apps that expect pasted command text to arrive atomically.
- The local socket API is more robust and secure: its Unix socket is now restricted to the current user, and long-running output waits and subscriptions stop cleanly on disconnect or shutdown instead of hanging indefinitely.

## [0.4.3] - 2026-04-07

### Fixed
- Update checks and in-app **What's New** release notes no longer depend on GitHub’s release API, which avoids the transient 403 failures from the previous update path.
- `herdr pane run ...` now submits the full command atomically in one request, fixing cases where scripted commands did not reliably execute because the final Enter was sent separately.
- Bare line-feed input is now preserved in raw terminal input instead of being normalized to Enter, fixing Linux terminal cases where inputs like Shift+Enter or Ctrl+J could be interpreted incorrectly.

## [0.4.2] - 2026-04-07

### Added
- The expanded sidebar agent panel can now switch between the current workspace and all workspaces, so you can scan and jump to agents across the whole session.
- The collapsed sidebar now shows compact per-pane agent indicators, so you can keep an eye on agent activity without reopening the full sidebar.

### Changed
- The sidebar now handles larger workspace sets more cleanly: the workspace section has headers, its own scrolling, better-aligned drag/drop slots, and manual width changes persist across restarts. Double-clicking the divider resets it to the configured default width.
- Pane scrollback is now configured with `advanced.scrollback_limit_bytes`, matching Ghostty's byte-based scrollback limit. Set it to `0` to disable pane scrollback entirely. The old `advanced.scrollback_lines` key is still accepted as an alias, but it now uses the same byte-based value.
- Linux release binaries now ship with libghostty SIMD enabled again without reintroducing the musl startup issue, restoring the optimized Linux build path.

### Fixed
- Typing in pane terminals on macOS is responsive again after the Ghostty migration, by keeping a persistent per-pane Ghostty key encoder instead of rebuilding it on every keypress.
- The collapsed sidebar expand toggle works again.
- Creating a new tab now waits until you confirm the dialog, so cancelling the new-tab flow no longer leaves behind an unwanted tab.
- Copying selected pane text now uses Ghostty's native selection extraction, which preserves wrapped text and wide characters more accurately.
- Session restore is more tolerant of older and current snapshot formats, including pre-tab session files.

## [0.4.1] - 2026-04-06

### Fixed
- Fixed Linux release binaries crashing on startup.

## [0.4.0] - 2026-04-05

### Major Changes
- Herdr now uses a Ghostty-backed terminal engine as its pane runtime.
- The legacy vt100 pane backend has been removed, making Ghostty the single terminal backend going forward.

### UX and Interaction
- Workspaces can now be reordered by dragging them in the sidebar.
- Notification sounds now support custom mp3 file overrides, with either one shared file or separate files for finished vs needs-attention alerts.

### API and Integration
- Workspace API ids are now stable, making socket and CLI automation more predictable across workspace changes and restores.

### Packaging and Runtime
- macOS builds now statically link the vendored `libghostty-vt`, preserving the single-binary install and update flow.

## [0.3.2] - 2026-04-03

### Changed
- The global launcher now surfaces update-related actions more clearly: when release notes are available you can open **What's New**, and when an update has been downloaded you can **quit to apply update** directly from the menu.
- Release notes are now retained as the latest available notes after you dismiss the startup modal, so you can reopen them later from the UI instead of only seeing them once.

### Fixed
- Fixed held-key repeat in terminal panes on macOS terminals that send explicit repeat events through the enhanced keyboard protocol, restoring continuous backspace, character, and arrow-key repeat without letting modal close/confirm key repeats leak into the shell.

## [0.3.1] - 2026-04-03

### Added
- New tabs now open directly into the rename flow, with the default tab name prefilled and replaced on first type so you can name tabs as you create them.

### Changed
- Polished modal layout and spacing across onboarding, settings, keybind help, and release notes so overlays feel more consistent and their content/actions line up more cleanly.
- Debug builds now use separate runtime/config paths from normal releases, which avoids local development sessions colliding with your main herdr install.

### Fixed
- Starting a second herdr instance against an active socket now fails fast with a clear error instead of clobbering the running session.
- Fixed pane and agent state updates being dropped under internal event queue pressure, which could leave a pane showing stale status after work finished.
- Fixed onboarding modal sizing and click targets, and corrected release-notes scroll calculations when a scrollbar is present.

## [0.3.0] - 2026-04-03

### Major Changes
- Added tabs within workspaces, so a single workspace can now hold multiple terminal tab contexts with their own pane layouts.
- Added first-class tab support to the local socket API and CLI wrappers, including `herdr tab ...` commands and tab ids like `1:2` alongside workspace-scoped pane ids.
- Added built-in direct integrations for pi, claude code, codex, and opencode, plus authoritative hook-driven state reporting so supported agents can report semantic state directly instead of relying only on screen heuristics.
- Added a post-update release-notes screen so herdr can explain what changed after an update is installed.

### UX and Controls
- Added optional direct pane-focus keybindings for terminal mode, so you can switch panes with modifier shortcuts like `alt+h` or `alt+right` without entering navigate mode first.
- Reworked keybind discoverability so the in-app keybind help now shows all supported actions, including optional bindings that are currently unset.
- Keybind help now uses a centered scrollable modal with mouse and keyboard scrolling, matching the release-notes interaction model more closely.
- Popups and action-button interactions now use more consistent modal geometry and button semantics across the UI.
- Polished the sidebar agent section so it focuses on detected agents only and uses clearer two-line agent cards with more breathing room.

### Behavior Fixes
- Hook-driven agent state updates now stay correct in tabbed workspaces.
- Modifier-only keypresses no longer leak into panes as stray input.
- Multi-tab agent labels now include tab names when that extra context matters.
- Workspace identity now follows the first tab's root pane again instead of stale creation-time cwd.
- Background notification suppression is now tab-aware rather than workspace-wide, so background tabs in the current workspace can still alert correctly.

### Documentation
- Updated the README, configuration guide, integrations guide, skill, and socket API docs to reflect tabs, direct integrations, unset optional keybindings, direct terminal-mode navigation examples, workspace-scoped pane ids, and the current workspace identity/sidebar model.

## [0.2.4] - 2026-04-01

### Fixed
- Fixed a macOS-only startup misdetection where pi could briefly appear as codex in the sidebar because process environment entries were being parsed as command-line arguments.

## [0.2.3] - 2026-03-31

### Changed
- Mouse wheel handling now follows the tmux/Ghostty model more closely: fullscreen apps receive wheel input when they own scrolling, while herdr keeps host scrollback for panes that are behaving like a normal terminal transcript.
- Pane scrollbars now only appear when herdr has real host scrollback for that pane, instead of implying a host-managed scroll position for app-owned scrolling.

### Fixed
- Fixed Codex and pi panes becoming unscrollable in herdr by preserving recoverable host history for top-anchored normal-screen output, without relying on alternate-screen scrollback retention.
- Fixed pane wheel routing so apps using mouse reporting or alternate-scroll behavior can receive scroll input directly instead of having herdr always intercept it.

## [0.2.2] - 2026-03-31

### Fixed
- Fixed pane scrollbars so they reserve their own lane instead of drawing over terminal content, which makes scrolling and scrollbar dragging behave more cleanly in narrow panes.
- Fixed alternate-screen scrollback handling so full-screen terminal apps can preserve recoverable history inside herdr panes instead of losing rows that scroll off.
- Fixed Codex in herdr panes losing transcript/history while running in alternate screen, so past output remains scrollable instead of disappearing as the session grows.
- Hid the rendered terminal cursor while a pane is scrolled back, avoiding stray cursor blocks appearing in the wrong place during history navigation.

## [0.2.1] - 2026-03-31

### Added
- Herdr now checks for updates at startup and periodically while it stays open, so long-running sessions can still discover new releases without a restart cycle.
- Added a lightweight bottom-right toast when an update has been downloaded and is ready, with a simple restart-to-use-it flow.

### Changed
- Rendering is now driven more directly by app events instead of relying as much on polling, which makes the UI feel snappier and cuts unnecessary redraw work.

### Fixed
- Restored smooth fast spinner animation for working agents.
- Closing a pane or workspace now reliably terminates the processes running inside that pane session instead of leaving shells or child processes behind.
- Fixed bracketed paste handling so incomplete paste sequences are preserved across read timeouts instead of being dropped or misread.

## [0.2.0] - 2026-03-30

### Added
- Added a local Unix socket API for controlling running herdr sessions, including workspace and pane management, pane reads, text/key input, pane splitting, and output waits.
- Added event subscriptions over the socket API for workspace and pane lifecycle events, pane output matches, and agent state changes.
- Added CLI wrappers on top of the socket API with `herdr workspace ...`, `herdr pane ...`, and `herdr wait ...`, using compact public ids for scripting and agent orchestration.
- Added a settings popup with mouse support for changing themes, sound alerts, and toast notifications from inside herdr.
- Added 9 built-in themes: catppuccin, tokyo night, dracula, nord, gruvbox, one dark, solarized, kanagawa, and rosé pine.
- Added interactive pane scrollbars, manual sidebar resizing, and upstream git ahead/behind indicators in the workspace sidebar.

### Changed
- Redesigned the sidebar into a two-section layout that separates workspace-level triage from per-agent detail, making it easier to supervise multiple agents in parallel.
- Agent state names exposed in the UI and integration surfaces now use `working` and `blocked`.
- Herdr now blocks nested launches by default when started inside a herdr-managed pane; set `advanced.allow_nested = true` to opt back in.

### Fixed
- Improved terminal keyboard protocol parsing and input forwarding across terminal variants, including better handling for shifted printable keys.
- Fixed Ghostty on macOS misparsing some arrow-key and modifier/enhanced key sequences.
- Refined sidebar rollups and pane ordering so workspace status and agent lists stay more stable and predictable.

### Documentation
- Refreshed the README, socket API reference, and reusable agent skill docs to better explain herdr's agent multiplexer model and integration surface.

## [0.1.2] - 2026-03-28

### Added
- Added first-run onboarding flow that lets you choose notification preferences (sound and toast) on startup.
- Added optional visual toast notifications in the top-right corner for background workspace events (completion and attention-needed alerts).
- Added configurable keybindings for all navigate mode actions: new workspace, rename workspace, close workspace, resize mode, and toggle sidebar. See the [configuration docs](https://herdr.dev/docs/configuration/) for the full key reference.
- Added configuration validation with startup diagnostics. Invalid key combinations or duplicate bindings now fall back to safe defaults with a visible warning.

### Changed
- **Breaking:** Default prefix key changed from `ctrl+s` to `ctrl+b` to avoid common terminal flow control conflicts.
- Workspaces now derive their identity from the repository or folder of their root pane, updating automatically as you navigate. Custom names act as overrides rather than static labels.
- Sidebar now shows workspace numbers again in expanded view.
- Refined sidebar presentation with consistent marker/name/state ordering and comma-separated agent summaries.
- Keybinding parser now accepts special keys (`enter`, `esc`, `tab`, `backspace`, `space`) and function keys (`f1`–`f12`).

### Documentation
- Split configuration reference into dedicated configuration docs with full keybinding documentation and config diagnostics explanation.

## [0.1.1] - 2026-03-28

### Added
- Added optional sound notifications for agent state changes, including a completion chime when background work finishes and an alert when an agent needs input.
- Added per-agent sound overrides under `[ui.sound.agents]`, so you can mute or enable notifications by agent instead of using one global setting. Droid notifications are muted by default.

### Changed
- Request alerts now play even when the agent is in the active workspace, while completion sounds remain limited to background workspaces.

### Fixed
- Improved foreground job detection on Linux and macOS so herdr can recognize agents that run through wrapper processes or generic runtimes, including cases like Codex running under `node`.
- Made Claude Code state detection more stable by handling more spinner variants and smoothing short busy/idle flicker during screen updates.

## [0.1.0] - 2026-03-27

### Added
- Initial release.
