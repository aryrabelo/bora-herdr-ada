# DOX framework — bora

- DOX is a self-documenting AGENTS.md hierarchy installed here (OMP-focused).
- Every agent must follow DOX instructions across any edits.

## Project

herdr is a terminal-based agent runtime for coding agents, written in Rust. Core surfaces: `src/app/` (server-side state and actions), `src/client/shell/` (the TUI, rendered from a `ClientShellSnapshot`; since herdr 0.9.0), `src/platform/<os>.rs` (OS-specific behavior), `src/detect/manifests/` (agent detection), `src/protocol/wire.rs` (server/client wire protocol), and the vendored `vendor/libghostty-vt`. Build, test, and validate through `just` recipes (`just test`, `just check`). Stable/preview both build from `master`.

## Core Contract

- AGENTS.md files are binding work contracts for their subtrees.
- Every meaningful change requires a DOX pass before the task is done: update
  the closest owning AGENTS.md when a change affects purpose, scope, ownership,
  contracts, workflows, constraints, or this index. Remove stale text
  immediately. Small no-behavior edits may leave docs unchanged — the pass
  still happens.
- Rules live in exactly one owning file. Child docs never restate parent
  rules. If two rules conflict, fix the docs in the same change —
  contradictions are bugs.
- Where a rule lives: operating rules → the owning AGENTS.md; long-form
  knowledge → docs/wiki; repeatable procedures → skills; machine-checkable
  rules → a script wired into the gate (see `enforcement.md` in the kit).
- Lessons learned in-session become rules with a date and a marker:
  `(learned YYYY-MM-DD, binding)`. A correction repeated ~3 times MUST be
  promoted to a dated binding rule — never keep re-correcting silently.

## Read Before Editing

Walk from the repository root to each path you will touch and read every
AGENTS.md along the route. The nearest AGENTS.md is the local contract; parent
docs hold repo-wide rules. OMP injects the root automatically and lists deeper
AGENTS.md as pointers — read the pointers before editing their directories.

## Scope and Audience

These instructions are layered.

- Unless a section explicitly says it is maintainer-only, local-machine-only, or
  external-contributor-only, treat it as universal project guidance.
- Universal project rules apply to every agent working on Herdr, including forks.
- Maintainer accounts are listed in `.github/MAINTAINERS`. Treat the acting
  account as a verified maintainer only when its username is listed there, the
  configured remote is the canonical `herdrdev/herdr` repository, and the
  authenticated account has write access to that repository. If any condition
  cannot be verified, skip maintainer workflow and follow the external
  contributor guardrail instead.
- Local Can machine workflow applies only on Can's own workstation or Windows
  VM setup, for example when `/home/can/Projects/herdr`, `HERDR_ENV=1`, or the
  `windows-wirt` SSH alias exists. If those facts are not true, skip local
  machine workflow.
- External contributor guardrail applies whenever the acting GitHub account is
  not a verified maintainer, the work is happening in a fork, or the account
  cannot be determined.

## Universal Project Rules

### Principles

- **State is separated from runtime.** `AppState` is pure data, testable without PTYs or async. `PaneState` is separate from `PaneRuntime`. Workspace logic doesn't need real terminals.
- **Render is pure.** `compute_view()` handles geometry and mutations. `render()` takes `&AppState` and only draws. Never mutate state during render.
- **No god objects.** If a module is doing too many things, split it. `app/` is already split into state, actions, and input. Keep it that way.
- **Platform code is isolated.** OS-specific behavior lives in the matching `src/platform/<os>.rs` file, with only shared traits, types, wrappers, and testable contracts in `src/platform/mod.rs`. Core modules don't have `#[cfg(target_os)]`.
- **Detection is decoupled.** The detector reads a screen snapshot, never touches the parser or viewport state.
- **Screen detection is evidence-based.** When changing `src/detect/manifests/`, first capture the relevant bottom-buffer state with `herdr agent read <pane> --source detection --format text` and, when styling or alternate screen behavior matters, `--format ansi`. Decide which visible controls are invariant, which are alternatives, and encode them as explicit AND/OR gates. Do not match whole-pane incidental text, and do not use the user-visible viewport for agent status because users can scroll it.
- **UI patterns should be reused.** Herdr is a mouse-first TUI. New dialogs, onboarding, settings, and post-update flows should follow the existing UI/UX language and interaction patterns instead of inventing one-off screens. Prefer reusing existing modal/screen structure, affordances, and close actions so the app feels consistent.
- **Layout changes must force a repaint, not just a re-render.** Any `AppState` mutation that reflows pane content (sidebar/right-panel toggle, or anything else that changes pane column/row allocation) without changing the outer terminal's `(cols, rows)` must explicitly signal a full repaint to every attached client. Both transport encoders (`ClientRenderState::TerminalAnsi`'s `BlitEncoder` and the default `SemanticFrame` client's local `BlitEncoder`) decide full-vs-diff repaint purely from whether the outer frame's dimensions changed; a layout change alone never trips that check, so the diff/scroll-shift path runs against already-reflowed content and desyncs the physical terminal from the encoder's model until an unrelated full redraw happens to fire. Route new layout-affecting mutations through `AppState::request_full_repaint()` (sets `force_full_repaint`, bridged into per-client `ClientRenderState::request_repaint()` in `HeadlessServer::render_and_stream`, and carried over the wire on `FrameData.force_full_repaint` for `SemanticFrame` clients) instead of assuming a dimension check will catch it. (learned 2026-08-13, binding: this exact gap caused a persistent, reproducible flicker — sidebar toggle open→close would desync the terminal until a workspace switch forced a full redraw — that survived two earlier throughput-focused render fixes because neither touched the full-repaint decision itself.)
  **Switching workspace and switching tab are in scope and were missed for months.** The rule above was written from the sidebar-toggle case and named only "sidebar/right-panel toggle", so the two mutations that reflow the ENTIRE terminal area — `AppState::switch_workspace` and `switch_workspace_tab` in `src/app/actions.rs` — went unrouted, and the bug reached the owner as "I have to click a workspace two to five times to switch". Every click worked: `self.active` changed, `workspace.focus` was logged each time, and three log lines 82ms apart for the same workspace id is what a user retrying a click that appears to do nothing looks like. Diagnosing it from the code alone is close to impossible, because the state transition is correct; the evidence that cracked it was the server log showing repeated successful focus events for one workspace, which says the input path is fine and the output path is not. Note the irony recorded in the original rule — a workspace switch was what accidentally repaired the sidebar-toggle desync — which is exactly why nobody suspected that a workspace switch had the same defect. When adding any mutation that changes which panes occupy the terminal area, assume it is in scope and gate the repaint on an actual change so re-selecting what is already active stays free. `toggle_zoom` and `close_pane` are the two remaining unrouted candidates; they are filed rather than fixed because there is no observed report for them and they may be covered by per-pane resize instead. (learned 2026-08-25, binding.)

### Prior art before building

Before any non-trivial feature reaches Rust, prove nobody already does it
well. The order matters: **grill the idea first**, until the destination is
sharp — a prior-art search run against a fuzzy destination returns everything
and decides nothing — then sweep these four shelves and write down what each
one returned, **including the searches that returned nothing**, because a
measured absence is a finding and the next session will otherwise search
again:

1. **GitHub at large** — is there a project that already solves this? Name
   stars, last commit, and licence, and say explicitly whether it solves it
   for a runtime like ours or for one we do not have (tmux, wezterm, a bare
   shell). A project whose whole design assumes tmux is a design reference,
   not a dependency.
2. **herdr plugins** — <https://herdr.dev/plugins/>, but the searchable
   directory is the GitHub topic `herdr-plugin` (**1040 repos**, measured
   2026-09-08 as 41 above 50★ + 65 in 11..50 + 538 in 1..10 + 396 at 0★, and
   the bands sum to the total — an earlier note here said ~250 and was wrong by
   4x); the site listing is a 30-minute auto-refresh with **no review of any
   kind**, so treat it as an index, never as a vetting signal. Sweep it in star
   bands, because GitHub's repo search caps a page at 100 and code search at 50
   and the low-star bands overflow silently. A plugin that already does it
   beats a core patch, and a core patch that could have been a plugin is fork
   merge-conflict surface bought for nothing (see Fork merge friction). Before
   concluding a feature must be core, check the plugin ceiling against the
   code: a plugin CAN hold a long-lived connection (`[[startup]]` is spawned
   detached and awaited without timeout, `src/app/api/plugins/runtime.rs`) and
   CAN draw in its own pane (`[[panes]]`), but CANNOT declare a sidebar
   band/view (`REGISTRY` is `const` in-binary and each band carries a compiled
   `push: fn(...)`) and CANNOT inject a workspace row the server does not
   have — its only sidebar channel is decorating an existing row with a
   metadata token.
3. **pi packages** — `https://pi.dev/packages?name=<term>`, one query per
   term. `omp` is of the pi lineage, so a pi package frequently runs on omp.
4. **omp plugins** — <https://github.com/topics/omp-plugin>. omp is the
   primary coding harness here, so a fleet/orchestration answer may belong
   there rather than inside bora at all.

Adoption has a **security gate**, not only a feature gate. For every
third-party candidate worth considering, state who maintains it, what it
executes at install and at runtime, which credential/socket/network it
reaches, and whether the code is auditable at the size it is. For a
herdr/bora plugin the bar is higher than it looks: the trust boundary is
INSTALL, not call — an enabled plugin receives `HERDR_SOCKET_PATH` and
therefore the reach of the entire CLI, unattended, at every server start (see
the plugin trust-boundary rule under Code Conventions). "Popular" is not an
audit.

The result of the sweep is written where the decision lives — the map ticket,
an ADR, or this file — never left in a chat.

(learned 2026-09-07, binding, owner instruction: *"temos que saber no GitHub
se não tem alguém que já faz isso bem, depois da gente fazer o grilling e
entender o que a gente quer"*, with the three plugin shelves and the security
requirement named in the same breath.)

### Multiplicative performance paths

Treat work reachable from view computation, rendering, background-pane resizing,
PTY parsing, detection, and client frame fanout as multiplicative. Before adding
work, identify its frequency and cardinality: per byte, event, or render × panes,
tabs, or workspaces × attached clients.

Inside pane-scaled render and layout loops:

- Use narrow terminal-state accessors. Do not collect aggregate input state,
  format terminal snapshots, inspect process trees, perform filesystem I/O, or
  allocate when one scalar fact is enough.
- Keep terminal-core lock duration minimal.
- Preserve hidden-source and retained-render early exits. Hidden panes still
  parse output, but their output must not trigger presentation work merely to
  keep terminal or detection state current.
- When a change adds or widens work in one of these loops, profile fixed geometry
  with 1 and at least 15 populated panes and report the scaling delta. Use
  `just bench-render-scale` to exercise both background-workspace and active-pane
  cardinality when applicable.

Prefer deterministic operation or architecture tests to wall-clock CI limits.
Performance benchmarks are supporting evidence, not substitutes for behavioral
coverage. Before a stable release, `just bench-release-smoke` must compare the
candidate with the current stable binary under hidden and visible output. When
the result moves materially or when validating performance work, repeat it with
`HERDR_PERF_SAMPLE_SECONDS=60` and investigate the affected scenario.

**The 0.9.0 sync moved visible-pane render cost from the client to the server, and
the smoke gate reads that as a regression.** Measured 2026-09-11 against stable
0.45.5 (macOS/aarch64, 60 s samples, two rounds): `hidden50` flat (+0.5%),
`visible30` +43% total CPU — server 1.07 → 1.96, client 1.00 → 0.64 (per-pid
split from `scripts/release_perf_case.sh`'s `cpu-raw.txt`; the smoke deletes its
results dir on exit, so run the case script directly with your own results path
to see it). The bare sync commit `27c65c27` (0.46.0, before any fork re-port)
shows the same +47%, so this is upstream's client-shell architecture (the server
now composes pane surfaces for the snapshot), not ceo-bora#275/#276 work. Running
the upstream `herdr` binary as the candidate does not work as an attribution:
the case script hardcodes the `bora/sessions` socket dir and its producer pane
fails with `writer pane 2 produced no output` — build the fork at a commit
instead. Do not re-run the smoke hoping for a different number; a stable release
across this line needs an explicit decision to ship with the note or to first
profile the server-side visible-pane path. (learned 2026-09-11, binding.)

### Runtime/client boundary guardrail

Herdr is migrating toward a server-owned runtime protocol with the TUI as one client. New work should not deepen the current server/TUI coupling.

Before adding state, API fields, events, commands, or socket messages, classify the feature:

- Shared runtime/session fact: belongs in server state and should be exposed through the JSON API/event path when practical.
- TUI presentation state: belongs only in the TUI/client layer.

Do not add new shared behavior that only works through the private TUI client socket. Use neutral server/API names, not UI-surface names like sidebar, row, card, or widget.

Examples:

- Pane/agent metadata, process state, terminal state, events: server/runtime.
- Sidebar layout, token placement, colors, selection, modals, mouse/viewport state: TUI/client.
- Workspace/tab/pane remain shared session organization for now, but avoid making them mandatory identity for unrelated runtime features.

### Fork merge friction — upstream sync

This fork must stay easy to merge with upstream `herdrdev/herdr`. The recurring conflict
classes, and how to resolve them, so the next sync is cheap:

- **Binary rename in user-facing strings.** This fork renames the `herdr` binary to `bora`
  in CLI output, docs, and config. Upstream merges reintroduce `herdr` in touched strings —
  grep the merged diff for `herdr` in string literals and rename to `bora`, but leave
  `herdrdev/herdr` repository/URL references and internal upstream identifiers alone.
- **Fork-only struct fields** (e.g. `change_set` on `WorkspaceGitStatusSnapshot`). Upstream
  restructuring a type we've extended produces a field-shape conflict. Keep the fork-only
  field, re-apply it to upstream's new shape, and re-verify its call sites compile.
- **Fork-only enum variants colliding with fixed-size arrays** (e.g. `Agent::Maki` in
  `SCREEN_MANIFEST_AGENTS: [Self; N]`). When upstream adds its own variant to the same enum,
  the array length and any exhaustive match/array-literal sites need both variants accounted
  for — search all `match`/array-literal sites of the enum after merging, not just the ones
  the diff flags.
- **Upstream bot identity (kangal) has no secret on this fork.** Upstream workflows use
  `secrets.KANGAL_GITHUB_TOKEN`, git identity `kangal-bot` (user id 285672167), and a
  matching user-id check in pr-gate.yml; none of it exists here (blamed to Ogulcan Celik,
  2026-05/06). The fork uses `${{ github.token }}` and `github-actions[bot]` (41898282)
  everywhere — on merges, swap any reintroduced KANGAL reference back, and keep the
  example text in the pre-release-audit prompts free of it.
- **`.github/{MAINTAINERS,APPROVED_CONTRIBUTORS}` are sync conflict surface, and losing
  the fork owner from them locks him out of his own repository.** `pr-gate.yml` runs on
  `pull_request_target` and reads BOTH lists from the DEFAULT BRANCH, not from the PR:
  an author who is in neither gets an "unsolicited implementation pull request" comment
  and the PR is closed automatically, in under ten seconds. That already happened here —
  `aryrabelo` was in neither list until `5116cec0` (2026-09-09 15:41Z), so ceo-bora#28
  and #29 were both auto-closed, and only #29 was recovered (a maintainer reopening is
  honored by the gate's own `hasVerifiedRecovery`, so reopen is the fix, never a force
  merge). The merge-friction part: upstream owns both files and the 0.9.0 sync
  (`27c65c27`) really did touch `.github/APPROVED_CONTRIBUTORS`, so a future sync taking
  `theirs` silently drops the fork's own entries and the lockout returns identically.
  On every upstream merge, diff both files and keep the fork's lines. The symptom does
  NOT read as a merge regression — it reads as "GitHub is blocking my PRs" — so check
  the two lists on the default branch before investigating anything else. (learned
  2026-09-10, binding.)

- **The 0.9.0 sync was a client rewrite, and it landed in two stages (ceo-bora#273,
  #274, 2026-09-08/09).** Upstream moved the entire TUI into `src/client/shell/*` +
  `src/client/endpoint/*`, rendered from a `ClientShellSnapshot`, emptying
  `src/ui/sidebar.rs` and `src/app/state.rs`. Stage 1 merged up to `8a6d6973` (the last
  commit before that wall, 38 conflicts); stage 2 merged `68c7b78e` taking `theirs`
  wholesale on the client shell and keeping the fork's SERVER/API/CLI surface. Fork UI
  (Folders view, chat view, context-menu plugin actions, timed hide) is being re-ported
  over `src/client/shell/` as ceo-bora#275/#276; its pre-merge implementation is at
  `6b6be670` (`git show 6b6be670:src/ui/sidebar.rs`), not in the tree. Consequences that
  outlive the merge: fork UI work goes in `src/client/shell/`, never back into `src/ui/`;
  the next sync is a plain `git merge upstream/master` again; and the fork's
  `Subscription` variants (`pane.result_reported`, `channel.message`) stay anchored ahead
  of upstream's four-variant tail, never appended past it — full analysis in
  `.local/prd/channel-identity-and-subscribe.md`.
- **After an upstream merge, every `dead_code` warning is a suspect disconnected fork
  feature until proven a leftover.** Stage 2 compiled and passed all 3377 tests with 32
  warnings, and three of them were fork SERVER features whose only call site lived in a
  file upstream had rewritten: the when-idle prompt drain tick (`bora agent prompt
  --when-idle` and `channel send --when-idle` queued forever — the one line
  `drain_settled_pending_agent_prompts` was gone from the headless tick), the pane input
  backpressure queue (upstream's `src/server/pane_input.rs` reintroduced the exact
  keystroke drop `6a15918a` had fixed), and the `idle_since` setter behind the
  `idle_seconds` API field that `hx-omp` sorts on (always null). None was caught by a
  test, because each test exercised the helper, not the tick or path that calls it. Rule:
  for each warning, `git show <pre-merge>:<file>` the deleted CALLER first; rewire when
  the observable contract is CLI/API, keep under `#[allow(dead_code)] // re-wired by
  ceo-bora#<leaf>` only when a filed re-port leaf owns it, delete only what upstream's
  client shell superseded. (learned 2026-09-09, binding.)

Every upstream sync also updates `UPSTREAM_HERDR_VERSION`/`UPSTREAM_HERDR_COMMIT` in
`src/build_info.rs` to the merged upstream tip, in the same commit as the merge — see
"Fork version identity" under Global Contracts.

(learned 2026-08-13, binding: these three classes caused the recurring merge conflicts in
today's upstream sync.)

- **Renamed paths in tooling, not just strings.** Upstream tooling hardcodes the `herdr`
  binary name and the `herdr` config-dir in places a string grep of the diff misses:
  release workflow asset checks (`bora.exe` vs the installer's staged `herdr.exe` — the
  Windows install ecosystem keeps `herdr.exe` internally, on purpose), justfile recipes
  (`target/release/herdr`), perf scripts (`$XDG_CONFIG_HOME/herdr/sessions/...` — the fork
  writes under `bora/`), and manifest tests asserting upstream asset filenames. After a
  sync, run the release pipeline end to end (`just pre-release-check` at minimum) before
  tagging; each of these fails only at release time, never in `just check`. (learned
  2026-08-19, binding: four such breaks shipped in one sync and each cost a failed release
  attempt — workflow version check, bench-release-smoke path, perf-smoke socket dir, and
  the windows asset-name test.)
- **New upstream test files arrive with the upstream binary name.** Upstream added
  `tests/broken_pipe.rs`, which calls `env!("CARGO_BIN_EXE_herdr")`; Cargo only defines
  `CARGO_BIN_EXE_bora` for this fork, so it's a hard compile error. This is the same rename
  class as user-facing strings, but it arrives in files the merge *adds* rather than files
  the diff touches, so grepping the merged diff for renamed strings doesn't surface it —
  after any sync, scan newly added files for the upstream binary name. Note that
  `tests/upstream_wiring.rs::no_source_file_references_the_upstream_binary_name` now catches
  this on any host.
- **Message-text drift is invisible to a macOS `just check` when only linux-gated tests
  assert it.** The 0.9.0 sync kept the fork's `agent_prompt_stalled` message ("no observed
  state change … state_change_seq remained …") while taking upstream's
  `tests/cli/agents.rs`, which asserts upstream's newer text ("no observed working or
  blocked state"); `tests/cli.rs` is one of the four `#![cfg(not(target_os = "macos"))]`
  files, so every local run and the macos CI leg were green and only
  `check (ubuntu-latest)` failed. Upstream's 8633a398 also DROPPED the variant's
  `baseline` field — port the text and the field together or neither, or the field goes
  dead. Same sync, same class: upstream's ci.yml brought the whole `windows-conpty-package`
  job with `herdr.exe` paths while this fork builds `bora.exe`; rename the paths, and gate
  the one step driving `windows_install_conpty_package_test.ps1` (asserts
  herdr.exe/herdr.cmd/PATH migration throughout, never fork-adapted) to
  `herdrdev/herdr`. The Windows ARM64 installer failure that predates this sync is the
  published-package side of the same rename: preview zips built before f87ef336 contain
  `herdr.exe` while install.ps1 expects `bora.exe` — the next preview publish heals it, no
  installer change needed. (learned 2026-09-09, binding.)
- **Upstream blocks conflicting wholesale where the fork moved code.** `src/ui/sidebar.rs`
  is roughly 5.8k lines in the fork versus 3.2k upstream, so git can't align them and
  produces one large conflict whose "ours" side is empty. Taking `theirs` would duplicate
  functions the fork already defines elsewhere (compile error) and resurrect code the fork
  deliberately replaced (e.g. `workspace_drop_slots`, superseded by
  `DragTarget::WorkspaceReorder`). The resolution that works: take `ours` to keep the fork's
  structure, then find the genuine upstream delta inside the block and port it by hand to
  every fork site with equivalent logic — in this sync that was one new function applied to
  three separate selection-background call sites. An `unused import` warning after
  resolving such a conflict usually means a real upstream delta was dropped; a merged
  upstream test that fails afterward points at the fork site still missing the port.
### Stable client endpoint contract

The client-owned TUI endpoint generation is independent from the private same-install protocol. Generation 1 is the compatibility floor for Local, SSH, and Cloud connections and must remain available unless retired for a security reason.

- Named core codecs are immutable. Do not add, remove, reorder, or reinterpret fields or enum variants reachable from a published codec. Introduce a new codec name and keep the old codec as a fallback instead.
- Keep baseline JSON handshake and snapshot fields required. New JSON fields must be optional or have field-specific defaults; new enum values need an `Unknown` fallback where older clients can safely ignore them.
- Add server features through advertised API methods and optional snapshot data when possible. A missing optional feature must disable only that action, not reject the connection.
- Do not change the meaning or load-bearing parameter shape of an advertised endpoint method. If an old server could ignore a new field and incorrectly report success, add a new method name or a separately advertised capability and omit that field without it.
- Missing methods, rejections, timeouts, and unavailable servers are client-local outcomes. They must not disconnect other compatible servers, and typing in a pane must not dismiss their notices.
- Frozen endpoint fixtures, bincode digests, wire-tag tests, and `tests/fixtures/endpoint-method-shapes-v1.json` are compatibility contracts. Never update a generation-1 expectation merely to bless a wire change; create and negotiate a new codec or method.
- Stable and preview update manifests advertise `endpoint_generation`. Keep release tooling aligned so an older updater knows when a new server generation really requires replacement.
- Existing-value digests cannot detect an appended enum variant. Review every enum reachable from a frozen codec as append-closed even when tests remain green.

## Maintainer Workflow

This section applies only to verified maintainers as defined under Scope and
Audience. Everyone else must skip this section and follow the external
contributor guardrail.

### Multi-agent isolation

Read-only investigation can happen in the shared checkout.

Small changes or small tasks are fine in the default main worktree. If you find unrelated implementation changes already in progress in the main worktree, use a dedicated worktree instead. Use a dedicated worktree for bigger features too.

Use this layout:

- shared integration checkout: `../herdr`
- task worktrees: `../herdr-worktrees/<task-slug>`
- task branches: `issue/<id>-<slug>` when an issue exists

Do all code edits, tests, and validation inside the task worktree.

Commit on the task branch in that worktree.

For substantive feature and bug-fix work, default to opening a pull request instead of pushing `master` directly. Small, low-risk changes and documentation-only updates can use a lighter workflow when Can prefers it.

Immediately before opening a pull request, fetch `origin` and make sure the task branch is based on the current `origin/master`; rebase it when behind, then rerun relevant validation before pushing. If `master` advances while the pull request is under review and GitHub marks it behind, update the branch and repeat checks and bot review on the new head.

After opening or updating a pull request, monitor all checks to completion with `gh pr checks --watch` or an equivalent command. Treat Greptile and CodeRabbit as part of CI: wait for both to review the latest pushed commit, not only for the build and test jobs to pass. Evaluate every actionable finding. Fix findings you agree with and reply with the fix; reply inline with a concise technical reason when you disagree. After any fix, wait for CI and both review bots again on the new head.

When the current pull request head is green and both bot reviews are complete, report that it is ready and stop. Never merge a pull request; Can performs the final merge.

If the current session is already inside an isolated task worktree, keep using it. Do not create nested worktrees.


After Can confirms the change is integrated, update the shared checkout, remove the task worktree, and delete the task branch locally and remotely.

## Verification

Use `just` recipes by default instead of invoking cargo or scripts directly.

```bash
just test               # cargo nextest + maintenance script tests
just check              # formatting check + cargo nextest + maintenance script tests
```

Run `just check` before committing unless Can explicitly accepts narrower validation. Do not bypass failing checks; fix the failure or explain exactly why a narrower check is enough.

**A fix is not in the operator's hands until `just install` runs, and a running
`bora` keeps the old binary.** `~/.local/bin/bora` is a plain file that
`just install` overwrites with a copy of the release build (since 2026-09-09 —
it was a symlink into the target dir before, see below), so it always looks
correctly installed no matter how old it is — which means a stale binary is
invisible from the outside and a report of "your change did nothing" can be
true of the binary and false of the tree. `just install` builds release, copies
it, and prints the version it just installed for exactly that reason: the
version string is the only cheap proof of what is actually on `PATH`. Because
`install(1)` replaces the file, an already-running `bora` holds the previous
inode and keeps running the old code until it is restarted or handed off. When
asking someone to verify a fix, ask for `bora --version` first. (learned
2026-08-25, binding: a repaint fix was reported as ineffective while the
installed binary was ten minor versions behind the tree, and the same stale
binary had earlier been suspected of *causing* a regression that shipped after
it was built.)
  **The same trap has a second mechanism: `[build] target-dir` in `~/.cargo/config.toml`.**
  This machine pins a single shared target dir (`/Users/aryrabelo/.cargo/target`, set in
  config.toml — NOT as an environment variable), so the install recipe's
  `${CARGO_TARGET_DIR:-target}` fallback always resolved to the checkout's own `target/`,
  and `just install` kept linking a weeks-old `target/release/bora` while every build
  landed in the shared dir: `cargo build` said 0.46.0, the installed binary answered
  0.45.39, and the tree was provably merged. The recipe now resolves the real location
  through `cargo metadata`'s `target_directory`, which honors both the env var and
  config.toml. If `bora --version` ever disagrees with `Cargo.toml`, compare
  `cargo metadata --no-deps` against the file you installed before suspecting the
  build. (learned 2026-09-09, binding.)
  **And a third, which is why the install is a COPY and not a symlink: the shared
  target dir means every worktree writes the SAME `release/bora`.** With the symlink
  form, an agent running `cargo build --release` in `worktrees/<x>/` — told, correctly,
  never to `just install` from there — replaced the installed 0.46.1 (wire protocol 24)
  with its WIP 0.46.2 (protocol 25) in place, and every CLI call on the machine failed
  with `protocol_mismatch` for 40 minutes while the server itself was perfectly healthy;
  `bora agent prompt` was among the casualties, so the orchestrator could not even tell
  the agent to stop. A copy decouples `PATH` from whatever built last. The rule for
  agents in worktrees follows: any release build there uses `--target-dir target`
  (inside the worktree), never the machine default, and proof-of-work binaries are
  invoked by path. (learned 2026-09-09, binding.)

**After landing a change, you can put the running server on the new build
yourself: `just install && bora server live-handoff`.** `live-handoff` hands
the running headless server over to the freshly installed binary without
killing panes or sessions, so the stale-binary trap above does not have to
wait on a human restart. Run `bora --version` after the handoff — the swap is
only proven when the reported version matches the tree you just built. Note
that a wire `PROTOCOL_VERSION` bump is the one thing the handoff does not
cover: the already-attached TUI client keeps the old protocol and must be
reopened. (learned 2026-09-01, binding, owner instruction.)

**Prefer `just install` from the main checkout, never from a linked worktree.**
Before the copy-based recipe, installing from `bora.worktrees/<x>/` symlinked
`PATH` to a directory built to be deleted, and harvesting that worktree left
`which bora` printing a path the shell answered `command not found` for, while
the server kept running — the symptom read as "bora stopped working" with only
the CLI gone. The copy removes the dangling-link failure, but a worktree
install still puts an unmerged tree on the operator's `PATH`, so the
preference stands. (learned 2026-09-04, binding: harvesting the `agente/112`
worktree after its merge broke the operator's `bora` command.)

**`just check`/`just lint` only lint the host target you run them on and cannot compile target-gated Rust from a macOS box; only CI's `ubuntu-latest` leg lints that code.** Two separate gating shapes hide code from a macOS run, and the second one is easy to miss: whole-file `#![cfg(not(target_os = "macos"))]` test files — derive the list, never copy it from here: `grep -rlE '^#!\[cfg\(.*not\(target_os = "macos"\)' src tests` (measured 2026-09-03 as four files; after the 0.9.0 sync it is two, `tests/auto_detect.rs` and `tests/cli.rs`, gated as `all(unix, not(target_os = "macos"))` — which is why the recipe's grep is a regex on the inner `not(target_os = "macos")` and not a fixed string, the earlier `-F` form went silent on that spelling). An incomplete list here is worse than none, because a test inside an unlisted file reads as "passing on macOS" when it was never compiled: an assertion in `api_ping.rs` was written, committed, reported green by a macOS `just check`, and first EXECUTED in `check (ubuntu-latest)`, where it failed — its author never had a way to run it, and platform modules excluded by an **outer** `#[cfg(target_os = ...)]` on their `mod` declaration in `src/platform/mod.rs` (`src/platform/linux.rs`, `src/platform/windows.rs`) — including their `#[cfg(test)] mod tests`. A green `just check` on macOS is not proof any of it is clean; `lint` prints a reminder naming all four files, and that reminder is a to-verify list, not noise. (learned 2026-08-13, binding: clippy failures in Linux-only-gated test files reached CI invisibly from a macOS `just check` this way. Reasserted 2026-08-22, binding: it happened again, and worse — `9a2db191` left `std::sync::{Mutex, OnceLock}` unused in `src/platform/linux.rs`'s test module and CI stayed red across three commits, because the reminder only grepped for the whole-file inner attribute and never mentioned the platform modules at all. The `lint` recipe now lists them explicitly. Cross-compiling to verify locally DOES work, and the earlier claim here that it does not was wrong: `LIBGHOSTTY_VT_PREBUILT=prebuilt/libghostty-vt-aarch64-macos.a cargo check --target <triple> --all-targets` runs in ~15s and exits 0 on both `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc` from an aarch64 macOS host. The env var bypasses the vendored libghostty-vt build script (which is what needs zig 0.15.2 while mise resolves 0.16.0), and `cargo check` never links, so the macOS-only static archive is never a problem. `--all-targets` is the load-bearing half: it compiles the `#[cfg(test)] mod tests` inside the platform modules, which is precisely what a macOS `just check` cannot see. Do this before pushing any change that touches `src/platform/`; CI is a 3-minute confirmation, not the only verifier. (learned 2026-09-03, binding: a one-line missing trait import — `.is_char_device()` needs `std::os::unix::fs::FileTypeExt` — survived the author, a `just check` exit 0 on macOS, AND an adversarial reviewer who cross-checked the WINDOWS target and found a genuine defect there, then died in `check (ubuntu-latest)` after a 3-minute wait. Checking one non-host target proves nothing about the others: check every triple the release builds.))

**A stale `target/` cache can pin an absolute path to a checkout that no longer
exists, and it fails at LINK time, so `cargo check` stays green and lies.**
Measured 2026-08-24: this checkout moved from `~/Sites/bora` to
`~/Sites/oss-team/bora`, and every test binary then failed with
`clang: error: no such file or directory: '/Users/aryrabelo/Sites/bora/prebuilt/libghostty-vt-aarch64-macos.a'`
while `cargo check --all-targets` reported zero errors, because `check` never
links. `build.rs` emits the prebuilt libghostty-vt path once and re-runs only
on `cargo:rerun-if-changed=prebuilt`, so nothing in a source edit invalidates
it. Fix: `touch prebuilt` and rebuild (or `cargo clean -p bora`). Read the
error's path, not the error's wording — the giveaway is a directory that is not
this checkout. (learned 2026-08-24, binding: a green `cargo check` is not
evidence the tree builds; only a command that links is.)

Unit tests live next to the code (`#[cfg(test)] mod tests`). New `AppState` or `Workspace` behavior should be testable with `AppState::test_new()` and `Workspace::test_new()` without PTYs.

For broad refactors or release-risk regressions, classify the risk before editing. Treat changes as refactor-risk when they touch two or more core surfaces, persisted state, protocol/API IDs, workspace/tab/pane identity, restore/handoff, agent detection authority, or UI/input state projection. Before moving code, identify the protected behavior and add or name characterization tests. Identity/state refactors should use the test-only invariants `AppState::assert_invariants_for_test()` or `Workspace::assert_invariants_for_test()` with adversarial state from `AppState::test_with_adversarial_identity_state()` or `Workspace::test_adversarial_identity_state()`. Run a roundtable for broad refactors and release-risk regressions, not for routine local fixes.

When testing a new Herdr build from inside an existing Herdr session, use
`cargo run -- ...` and clear inherited Herdr socket overrides so the debug
binary talks to the debug `herdr-dev` server instead of the installed stable
server:

```bash
env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH cargo run -- <command>
```

**Integration tests that spawn a `bora server` must scrub every ambient `HERDR_*`
variable, and `HERDR_STARTUP_CWD` is the one that bites.** Three upstream tests
(`api_pane_output_is_fanned_out_as_pane_surface_updates`,
`same_tab_geometry_follows_meaningful_client_activity`,
`federated_client_starts_without_local_and_survives_its_restart`) failed only when
`just check` ran inside a bora pane and were green on CI: the pane's shell carried
`HERDR_STARTUP_CWD=$HOME`, the test server inherited it and seeded an extra
workspace the test never asked for. The variable was in the pane's environment
because `run_server` took it out of the env AFTER `App::new`, which restores the
session and spawns every pane shell — fixed in ceo-bora#318 (bora #39) by taking it
first. Every test harness now scrubs `HERDR_STARTUP_CWD` next to `HERDR_ENV`; when
adding a spawn site, copy the whole scrub block (`tests/api_ping.rs`
`spawn_herdr_with_env` is the reference), and when a test fails only on this
machine, bisect the `HERDR_*` env before reading the test. (learned 2026-09-11,
binding.)

**Trialling a third-party plugin: `--session` is NOT isolation, and two clones
are NOT two builds.** Three facts measured 2026-09-07 while running the
`herdr-mirror` plugin against a second machine, each of which silently
invalidates the obvious safety plan:

- **The plugin registry is global per NAMESPACE, not per session.**
  `registry_path()` is `config_dir().join("plugins.json")`
  (`src/persist/plugin_registry.rs`) and a named session only moves the *data*
  dir (`src/session.rs`), never `config_dir()`. So `plugin link` inside
  `bora --session throwaway` writes into the registry the LIVE session reads,
  and its `[[startup]]`/event hooks then run in the live server. Isolation
  requires `HERDR_NAMESPACE=<name>` on every command; verify by hashing
  `~/.config/bora/plugins.json` before and after. Also clear
  `HERDR_ENV`/`HERDR_SOCKET_PATH` from the environment: the first blocks a
  nested bora (`src/main.rs`) and the second makes `--session` a no-op
  (`src/session.rs`).
- **`CARGO_TARGET_DIR` is set globally on this machine (`~/.cargo/target`), so
  two clones of the same crate share one `target/release/<bin>`.** Building
  clone B overwrites clone A's binary with no warning, and a trial that
  believes it is exercising the fork can be measuring upstream. Pass an
  explicit `--target-dir` per clone, and prove which build you have by probing
  the binary for a symbol only one side contains — not by its path.
- **There is no `bora server start`.** `bora server` runs headless in the
  foreground; the daemon is otherwise spawned only by the TUI launch path
  (`spawn_server_daemon`, `src/server/autodetect.rs`), and a plain CLI verb
  does NOT start one — it fails with `server_not_running` and stops. On a
  headless remote host the equivalent is
  `nohup bora server </dev/null >/dev/null 2>&1 &`, which is exactly what
  `build_server_daemon_command` does. Non-interactive SSH also drops
  `~/.local/bin` from `PATH`, so anything naming the binary remotely needs an
  absolute path.

### Rules-review gate

`.github/workflows/independent-review.yml` runs `scripts/review_rules.py` on
every push to a PR (`pull_request: synchronize`), diffing `base...head` so it
reviews the pushed commit rather than the author's working tree. The reviewer
is deterministic — a stdlib-only Python script, no model call, no credential —
and enforces four diff-scoped rules that are already binding elsewhere in this
file but that a lint or unit test cannot express, because each is about *the
change* rather than the code state: the version bump on `Cargo.toml` package
changes, the generated/published-path restriction (root `README.md`/
`CHANGELOG.md`, `docs/preview/`, `docs/versions/`), the required justification
comment on `#[allow]`, and the ban on GitHub closing keywords in commits.
Findings **block the merge**: they are violations of a written rule, not a
model's opinion, so unlike the old gate they are not advisory.

Two rules already have their own dedicated, more thorough checkers and are
deliberately *not* duplicated here: `unwrap()` in production is
`clippy::unwrap_used`, and root-vs-`docs/next` changelog divergence is
`scripts/changelog.py check-history-sync`. Generated output that arrives through a merge is exempt when its blob at HEAD is
byte-identical to a non-first parent of a merge commit in `base..head`
(`merge_inherited_paths`): an upstream sync brings herdr's release-CI output
(`docs/versions/<v>/`) untouched, and that is inherited, not hand-edited — while
any local edit on top changes the blob and fires again. The fork's own
`docs/preview/` snapshot is kept as `ours` on a sync for the same reason: Preview CI
owns it and regenerates it from `docs/next` on the next run. (learned 2026-09-09,
binding: the 0.9.0 merge hit 107 criticals on paths nobody had edited.)

When a rule is ambiguous on a given
diff, the checker does not flag it — mass false positives are what make a team
learn to ignore a gate.

Run it locally before pushing with:

```bash
BASE_SHA=main HEAD_SHA=HEAD scripts/review_rules.py
```

(learned 2026-08-17, binding: measured across one review session, deterministic
checks and plain execution of the rules above found four real defects while the
prior independent-model reviewer found one, at roughly ten minutes and a
per-push model-call cost. The model gate is retired in favor of these
deterministic checks for that reason.)

## Local Can Machine Workflow

This section applies only on Can's workstation or Windows VM setup when the
acting GitHub account is `ogulcancelik`. Other verified maintainers skip this
local-machine section but continue following maintainer workflow. Everyone else
follows the external contributor guardrail.

### Windows VM validation

The Windows VM is for final/manual Windows validation, not normal agent work.
Connect to it with the `windows-wirt` SSH alias.

Use the single reusable checkout at `C:\work\repo`. Do not create additional
persistent Herdr clones or worktrees on the VM. The Windows account is already
named `herdr`, so avoid paths like `C:\Users\herdr\herdr`.

Before validating a fix on Windows, sync or apply the Linux worktree changes
into `C:\work\repo`, then run the needed Windows build or test commands there.
Reuse the shared Rust caches under `C:\Users\herdr\.cargo` and
`C:\Users\herdr\.rustup`. Do not use WSL on the VM. The VM may have a newer
Zig on `PATH`; Herdr currently requires Zig 0.15.2, so set
`$env:ZIG = "C:\Users\herdr\zig-0.15.2\zig.exe"` before running Cargo commands
that build the vendored libghostty-vt.

After validation, leave `C:\work\repo` clean. Remove temporary files and delete
`C:\work\repo\target` when disk space is tight, but keep the shared Cargo and
Rustup caches. Unless Can explicitly asks to keep the patched tree for more
manual testing, reset `C:\work\repo` back to a clean checkout before finishing.

## Agent Detection Updates

Agent detection changes should use the manifest hot-reload loop. Use the project-local `herdr-throwaway-repro` skill to create a disposable named session and drive the real agent UI through Herdr's CLI/API into the target state. Read the pane with `herdr agent read <pane> --source detection --format text` and inspect matching with `herdr agent explain <pane> --json`. Update the bundled manifest in `src/detect/manifests/<agent>.toml`, copy that manifest to the local override path at `~/.config/herdr/agent-detection/<agent>.toml`, then run `herdr server reload-agent-manifests` against the session under test. Before writing the override, check whether one already exists; never overwrite or remove a pre-existing override without alignment. Once the rule is correct, remove the temporary override or restore the previous one exactly so the committed bundled manifest remains the source of truth.

Do not add large agent-specific full-screen fixture suites for routine manifest tuning. Keep Rust tests focused on manifest parsing, rule semantics, skip-state semantics, source precedence, cache reload behavior, and update flow. Use live pane reads for agent-specific screen evidence.

`distribution/agent-detection/` is the remotely published catalog for released clients. Keep changes for already released agents aligned with their bundled manifests unless the validator records an exact compatibility exception. A newly bundled agent that current stable clients cannot identify may remain unpublished behind an exact version-and-digest exception, but it must be added to the catalog and the exception removed before the first stable release that ships it. `just release-docs-check` enforces that no unpublished exceptions remain.

## Vendored libghostty-vt

`vendor/libghostty-vt.vendor.json` records the upstream source commit currently vendored.

Local patches on top of the vendored source must be tracked in `vendor/libghostty-vt.patches.md` and stored as patch files under `vendor/patches/libghostty-vt/`. Each entry should say why the patch exists, the Herdr issue, upstream PR/discussion, vendored base commit, touched files, verification, and the exact removal condition.

When updating libghostty-vt, check every active patch in `vendor/libghostty-vt.patches.md`. If the new upstream commit contains the fix, remove the local patch and index entry, then rerun the listed verification. If not, reapply the patch on top of the new vendored source.

`just check` runs maintenance tests that verify local libghostty-vt patch files are listed in the index and reverse-apply cleanly against the vendored tree. Do not leave a patch file untracked or an indexed patch unapplied.

## Docs

`skills/herdr/SKILL.md` tracks the latest stable Herdr release because the unversioned `npx skills add herdrdev/herdr --skill herdr -g` command installs it from `master`. Do not update this file in feature or preview work. Review and update it only during stable release preparation, and include the change in the release commit with the `Cargo.toml` version bump. Preview builds keep the latest stable skill.

Unreleased docs live in `docs/next/website/src/content/docs/`. Update those when a user-facing change needs docs before the next release. They are committed drafts but are never production website input. `docs/next/README.md` and `docs/next/CHANGELOG.md` stage root README and changelog changes: **`docs/next/CHANGELOG.md` is the single source of truth for unreleased entries.** Append new entries only there. Root `CHANGELOG.md` stays release-generated: its `## Unreleased` section must stay empty between releases, and `just release-prepare` promotes `docs/next/CHANGELOG.md`'s Unreleased section into a new versioned entry in both files (`scripts/changelog.py prepare --path docs/next/CHANGELOG.md`, then that result is copied over root `CHANGELOG.md`, never the other direction). `just release-docs-check` (and therefore `just pre-release-check`/`just release-prepare`) runs `python3 scripts/changelog.py check-history-sync`, which fails loudly instead of silently overwriting either file when root has direct Unreleased content or when released history has diverged between the two files — reconcile by hand before releasing if it fires. (learned 2026-08-13, binding: an earlier version of `release-prepare` copied root into `docs/next` and destroyed docs/next-only content; do not reintroduce that direction.)

The active preview release docs live in `docs/preview/website/`. Preview CI owns this mutable snapshot and commits it atomically with `distribution/preview.json`; never edit it manually. Validate it with `node scripts/docs/preview.mjs check`.

Published stable-release documentation lives in `docs/versions/`. Release CI seeds each version from the tagged `docs/next` tree, and maintainers may correct factual documentation errors in a published version afterward. Apply a correction separately to `docs/next` when it also applies to future releases; never replace a published tree with the current draft. The private website renders `/docs/preview/` from the active preview snapshot, `/docs/<version>/` from the maintained version directories, and `/docs/` from the version selected by `docs/versions/manifest.json`. Herdr remains the source of truth for the public snapshots.

During release review, finalize `docs/next` and run `just release-docs-check`. Do not copy draft docs into preview or published versions manually. Preview CI snapshots the selected commit. After a stable GitHub Release succeeds, release CI seeds a new version from the exact tag and updates `distribution/latest.json`. The resulting master commit triggers the private website deployment.

Normal feature and fix work must not edit `docs/next/CHANGELOG.md`; this keeps long-lived branches from conflicting over one shared release file. When refreshing an older pull request, remove its changelog-only diff. Keep user-facing commit subjects descriptive and include required `refs #<issue-number>` lines so stable release preparation can inventory the full range. During the pre-release audit, use that inventory to human-write and curate the user-facing entries in `docs/next/CHANGELOG.md`; generated commit lists are source material, not final release prose. Do not add changelog entries for website-only, documentation-only, CI, build-pipeline, or repository-maintenance changes.

Normal feature/fix work should not edit root `README.md`, root `CHANGELOG.md`, published version docs, or `distribution/latest.json` unless it is a focused correction to already-published documentation or explicitly requested.

Put local PRDs, planning notes, and exploratory specs under `.local/prd/`; `.local/` is ignored and locally controlled.

## Global Contracts

### Version Bump

Every shipped update bumps the bora version. When a change lands in `Cargo.toml`'s
package, bump `version` in the same commit — never ship code without a version bump,
because the installed binary reports `bora --version` and an unbumped build is
indistinguishable from the previous one at runtime. (learned 2026-08-14, binding:
requested directly by Ary after an update shipped under the old version number.)

### Fork version identity

The fork's human-facing version is `v<upstream herdr version>[<upstream commit>].bora-<our
minor>`, rendered by `build_info::fork_version_display()`. `UPSTREAM_HERDR_VERSION` and
`UPSTREAM_HERDR_COMMIT` in `src/build_info.rs` must be updated in the same commit as any
merge of `upstream/master`: set them to the upstream release the merged tip belongs to and
the short SHA of that merged upstream tip — never the fork's own merge commit, which says
nothing about herdr. `BASE_VERSION` and `version()` stay plain semver on purpose: update
checks (`update::Version`), the wire protocol `version` field, live-handoff acceptance, and
seen-state storage keys all compare them, and none of those comparisons may see the fork
suffix. (learned 2026-08-19, binding.)

### Commit Style

Use lowercase conventional commits, no emojis, and no AI co-author lines. Commit subjects feed preview release notes, so keep them descriptive.


When a normal feature or fix commit relates to a GitHub issue, add a commit body line `refs #<issue-number>` after the subject:

```text
fix: handle pane focus

refs #82
```

Do not use GitHub closing keywords like `fixes #<issue-number>`, `closes #<issue-number>`, or `resolves #<issue-number>` in normal commits. `master` contains unreleased work; release CI closes referenced issues after the GitHub Release is created.

### Code Conventions

- Rust: no `unwrap()` in production code. Use `tracing` for logging. Use `#[allow]` only with a comment explaining why.
  **This is enforced, and where it is enforced matters.** `just lint` runs clippy twice: once
  `--all-targets` with `-A clippy::unwrap_used`, then once `--bins` with
  `-D clippy::unwrap_used`. Do NOT move this into `Cargo.toml`'s `[lints.clippy]` — that table
  has no per-target scope, so a `deny` there also hits the hundreds of legitimate `unwrap()`
  calls in test fixtures that the `--all-targets` run compiles, and `just check` goes red on
  code that was never in scope. `--bins` does not compile `#[cfg(test)]` modules, which is
  exactly the production scope this rule names. `scripts/windows_check.ps1` mirrors it.
  Two measurement traps make this rule easy to declare "clean" while blind: **clippy does not
  re-emit warnings from a cached build** (`touch src/main.rs` first, or read a stale zero), and
  **`--message-format short` omits the lint name**, so grepping that output for
  `clippy::unwrap_used` matches nothing. Count with `--message-format json` and read
  `message.code.code`. (learned 2026-08-24, binding: the rule sat unenforced with 48 production
  violations while both traps independently produced a confident zero during the cleanup.)
- Rust platform-specific code must be compile-gated. Put OS APIs and substantial OS behavior in `src/platform/`; when platform checks are needed elsewhere, use `#[cfg(windows)]`, `#[cfg(unix)]`, or target-specific `#[cfg(...)]` on imports, fields, functions, impls, and match arms so Windows-only code does not compile into Unix builds and Unix-only code does not compile into Windows builds. Use `cfg!(...)` only for pure cross-platform policy constants whose branches both compile on every target.
- Don't add dependencies without a reason. Check whether existing dependencies cover the need first.
- Integration asset versions (`HERDR_INTEGRATION_VERSION` markers and matching `*_INTEGRATION_VERSION` constants) are migration versions relative to the latest released tag, not per-commit counters on `master`. If an integration asset changes multiple times between releases, bump it once from the version in the latest release.
- When changing the server/client wire protocol, compare `src/protocol/wire.rs::PROTOCOL_VERSION` against protocols published in both stable and preview releases. Bump it when the current source protocol has already been published in either channel and the wire format changes incompatibly. Do not bump it again for multiple incompatible changes before that protocol is published. Update hardcoded protocol expectations and manual protocol fixtures in tests.
- Adding an `EventKind` is two lists, not one. A new variant reaches `events.subscribe` as soon as it is in `EventKind` with a `Subscription` arm, but a plugin manifest `[[events]] on = "..."` hook stays inert until the variant is also in `PLUGIN_HOOK_EVENT_KINDS` (`src/api/schema/events.rs`), because `run_plugin_event_hooks` returns early on any kind missing from that list (`src/app/api/plugins/runtime.rs:220`). The failure mode is a silent no-op: the manifest parses, the plugin installs, the hook never fires, and nothing logs. Give every new kind its own arm in the plugin-context match too — one that resolves a real workspace instead of falling into `empty_plugin_context` — or the hook fires with no context to act on. `layout.updated` is deliberately subscribe-only and is the counter-example, not the template. (learned 2026-08-17, binding: `github.pr_opened` was added this way; a mutation run confirmed that dropping the `PLUGIN_HOOK_EVENT_KINDS` entry is caught only by a test asserting membership directly, and that deleting the `emit_event` call is caught only by a test asserting the event is emitted. `github.pr_opened` and its subscribe-only sibling `github.prs_refreshed` — the original counter-example here — were retired with the GitHub polling in ceo-bora#271, 2026-09-08; the two-list rule is unchanged.)
- Adding a `Subscription` variant is the mirror of the rule above, and its blind spot is one layer deeper. Three lists must agree — the `Subscription` enum, `ActiveSubscription::new`'s arm (`src/api/subscriptions.rs`), and `subscription_for_name` + `DEFAULT_EVENT_NAMES` (`src/cli/events.rs`) — plus the generated artifact `docs/next/api/herdr-api.schema.json`, which is gated by `generated_protocol_schema_artifact_is_current` and regenerated with `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current` (the exact form the gate's own panic message prints — `src/api/schema/tests.rs`; a `cargo test --bins` invocation also works but is not the canonical one anyone will be told to run). Dropping the name is caught loudly. **Pointing the arm at the WRONG `EventKind` was caught by nothing**: a mutation run swapping `EventKind::ChannelMessage` for `EventKind::LayoutUpdated` in the `channel.message` arm left the whole suite green, and a subscriber would have received somebody else's events with no error on any side. Assert the pairing against the WIRE STRING, not the variant — `active.event_kind.dot_name() == "<the subscription's own serde rename>"` — because that is what a client actually asks for; `channel_message_subscription_uses_the_matching_event_kind` is the pattern, and `workspace_metadata_subscription_uses_dedicated_event_kind` is the weaker `matches!` form that predates it. Note also that a count literal like `DEFAULT_EVENT_NAMES.len() == 26` belongs in exactly ONE test (the guard against accidental changes to the user-visible default stream); every other consumer derives from the const, or a one-line addition reddens unrelated tests for nothing. (learned 2026-09-08, binding: found by mutation while adding `channel.message`, which the server had been emitting to plugin hooks all along with no way for `events.subscribe` to express it.)
- For a `placement = "tab"` plugin pane, `open_plugin_tab` (`src/app/api/plugins/panes.rs`) applies the manifest `[[panes]] title` as the new tab's custom name via `set_custom_name` + `crate::logging::tab_renamed`, mirroring `handle_tab_create` in `src/app/api/tabs.rs`. A plugin must not follow `plugin pane open` with a `bora tab rename` call for tab placement — that is dead code now, though the third-party `persiyanov.reviewr` plugin still does it. (learned 2026-08-20, binding.)
- `agent start` does not `exec` the agent binary — it types the executable name as literal shell text into the target pane's already-running interactive shell (`src/app/agents.rs`, via `interactive_shell_command`). That shell is the user's real login shell with rc files sourced, so a local shell function or alias of the same name INTERCEPTS the launch. Consequence: agent launch behavior depends on the operator's rc files, and a wrapper can silently change what runs. This is why `omp` on a machine whose zshrc defines an `omp()` sandbox wrapper launches sandboxed. It is also why `[agents.commands]` can point a kind at a shell ALIAS (`omp = "omp-raw"`): an alias resolves precisely because the name is typed into an interactive shell rather than exec'd, and the same override would be impossible under `exec`. When debugging "the agent launched but behaves wrong", check the operator's rc files before the Rust. (learned 2026-08-21, binding: a full investigation concluded "no-op, bora already execs the raw binary" and was wrong, because the spawn path is a shell write, not a process exec.)
- A unit test that calls `mcp::tools::dispatch` (or anything else that opens the API socket) runs against **whatever bora server is live on the developer's machine**, not against nothing. Several existing tests get away with it because they assert `DispatchError::Tool(_)` and the verb they use fails fast, but the pattern is unsound in two ways and both were shipped and caught in one session: a long-poll verb (`events.wait`, `channel.wait`, `agent.wait`, `pane.wait_for_output`) with no `timeout_ms` blocks forever, turning a 35-second `just check` into an indefinite stall that looks like a hung build rather than a red test; and a side-effecting verb (`agent.start` above all) actually performs its effect in the operator's own session. Test the pure decision instead — extract the branch under test into a function that takes its inputs explicitly and assert on that. When a dispatch-level test is genuinely the point (proving `dispatch` wires a check at all), it MUST use a verb and params that cannot reach past the check, and it SHOULD pass `timeout_ms` anyway so that removing the check produces a fast failure instead of a stall. (learned 2026-08-22, binding.)
- The sidebar's three-pass lockstep contract is `entry_row_height`'s doc comment in `src/ui/sidebar.rs` (find it by symbol; earlier notes cited a line range that has since drifted onto an unrelated function). `workspace_list_visible_count`, `compute_workspace_list_areas`, and `render_workspace_list` MUST all derive every row height by calling `entry_row_height` — never a local constant, never inline arithmetic. `workspace_list_lockstep_passes_agree_for_every_entry_variant` is the characterization test for it and uses non-wildcard `match`es on `WorkspaceListEntry`, so a new variant fails to compile until it is handled there. Note that every variant except `PaneDotsRow` is height 1 (that one is 2 rows in the split shape, 1 inline, plus a trailing gap), so for the flat variants the test cannot be falsified by changing a height — the only mutation that reddens it is making ONE pass disagree with the others. Its value is prospective: it is the net under any change that introduces a row taller than one line. (learned 2026-08-22, binding.)
- A test-only guard that mutates a process-global env var while holding `test_config_env_lock` MUST be exactly one type. It was briefly split per variable (`IsolatedStateDir`, `IsolatedConfigDir`), and the first test that needed both — `project.create`, whose file lives under `config_dir()` and whose channel binding writes a roster under `state_dir()` — constructed both and deadlocked forever, because that lock is a plain non-reentrant `parking_lot::Mutex`. Nothing in the type system prevents nesting, and the symptom is not a red test but a hung suite: a 22-second `just check` became a 30-minute timeout that reads like a broken build. `crate::config::IsolatedDirs` now isolates both variables in one guard, so there is no second guard to nest; isolating a variable a test does not care about costs nothing. Do not re-split it, and apply the same rule to any future env-var guard. (learned 2026-08-22, binding.)
- **A plugin's trust boundary is install time, not call time**, and that is an accepted risk rather than an oversight. `src/api/server.rs` dispatches on `request.method` with no per-caller capability check, so a plugin subprocess has exactly the reach of the `bora` CLI — `workspace.*`, `worktree.*`, `pane.send_input`, `agent.start/prompt`, `channel.*`, `server.reload_config`, all of it. `HERDR_SOCKET_PATH` is set unconditionally in the shared `start_plugin_command` (`src/app/api/plugins/runtime.rs`), which every entry point funnels through: startup hooks, event hooks, actions and panes alike. Since `run_plugin_startup_hooks()` runs at every server start (`src/server/headless.rs`, right after `print_ready_message`) with **zero human interaction**, installing an enabled plugin already grants arbitrary unattended full-wire-API execution. The consequence for design: do NOT add a capability gate to one newly-exposed invocation path while that one stays open. A right-click menu item firing a plugin action requires an explicit human click and is therefore *strictly more* gated than `[[startup]]`; gating only the menu would add friction exactly where the human is present and none where they are absent, which is theater rather than security. Narrowing the boundary itself is tracked separately (a per-plugin scoped RPC table, the one idea worth taking from `deepseek-ai/deepseek-harness`); when that lands, this rule stops being the whole story and must be updated in the same change. Prior-art comparison in `.local/prd/plugin-extensibility.md`. (learned 2026-08-24, binding: this was resolved as the blocking prerequisite of the plugin-menu work, by measuring what already runs unattended rather than by assuming the new path was the riskiest one.)
- **A doc comment promising that consumers "can never drift apart" is not a mechanism; owning the whole classification is.** `check_status.rs` owned only the *failing* conclusion set (`is_failing_conclusion`) and let each caller infer everything else from it. Its comment said the sharing existed so the consumers "can never drift apart" — and they had drifted by the time a fourth consumer appeared, because inferring two states from one predicate is not a shared rule, it is three separate guesses. Concretely: `checks_rollup` derived Passing by elimination, so a `COMPLETED` check with `conclusion: null` (which GitHub really emits) and a `COMPLETED` check carrying any conclusion string added to the API later BOTH displayed as a green tick, and `checks_counts` counted them as passing. Neither case had a test. The fix is `run_state(status, conclusion) -> ChecksRollup` as the single owner, with `check_run_state`/`reduce_run_states` exposed so `open_prs`'s `statusCheckRollup` reduction goes through it too, plus a test asserting the counts/rollup invariant across the whole status x conclusion cross product rather than promising it in prose. When you find yourself writing "so these can never disagree", check whether the code makes disagreement impossible or merely unlikely. Also: unrecognised input must never classify as the optimistic value — green is a claim about someone else's CI, pending is an admission of ignorance, and only one is safe to be wrong about. (learned 2026-08-24, binding: found by a subagent sanity-checking its own new code against the existing path, which is the only reason it surfaced at all.)
- **Since the 0.9.0 sync the headless server's tick is the ONLY tick, and it is where fork tick work silently dies in a merge.** `HeadlessServer::handle_scheduled_tasks_headless` (`src/server/headless.rs`) is the sole scheduled-task path — upstream removed `App::handle_scheduled_tasks` with the client-shell rewrite, so the old rule about two ticks drifting apart no longer has a second tick to drift from. The failure mode moved instead of disappearing: any fork tick work is now one line in a file upstream rewrites on every sync, and stage 2 dropped exactly that line for the when-idle prompt drain (see the dead-code rule under Fork merge friction). Tick work MUST live in a named helper on `App` (`drain_settled_pending_agent_prompts` is the pattern) called from that tick, with a behavioural test at the headless level (`src/server/headless/tests/mod.rs`: enqueue → advance the tick → observe delivery) so that losing the call site reddens a test rather than a feature. (learned 2026-08-25 as the two-tick drift rule — a store poll shipped only in the App tick and never reached server mode; re-learned 2026-09-09, binding, in the single-tick shape.)
- **`grep` on this machine is a uutils reimplementation, and it parses `()` as an empty regex group.** So `grep -c "unwrap()"` matches every `unwrap` substring — `unwrap_or_default()`, `unwrap_or_else()` — and over-reports. Measured on `src/ui/sidebar/project_view.rs` in one session: 65 unescaped vs 52 with `grep -c 'unwrap\(\)'` or `grep -cF`. Always `-F`, or escape the parens, when counting anything with a call suffix, and the same applies to `todo!()`/`unimplemented!()`. Two neighbouring traps in the same family, both of which produced confident wrong numbers in the same session: a substring check for `TODO` matches the sidebar band name `TODOS`, which reported 19 phantom TODO comments against a real count of 0; and checking whether an `#[allow]` is justified by inspecting the PRECEDING line misses every one justified by a trailing comment on the same line, which reported 8 unjustified against a real count of 0. This sits alongside the two clippy traps already noted above (no re-emit from a cached build; `--message-format short` drops the lint name) — the general rule is that a proxy measurement must be falsified against a known-good and a known-bad input before its output is quoted anywhere. (learned 2026-08-24, binding.)
- **A whole-frame hash golden is updated by ATTRIBUTION, never by bumping.** `desktop_full_app_semantic_frame_is_characterized` and its mobile sibling (`src/ui/tab_surface.rs`) assert a SHA-256 of the encoded `FrameData`, so any intended rendering change breaks them and the hash itself tells you nothing about what changed — which means pasting the new digest is indistinguishable from pasting over a real regression. The technique that makes the update honest, and which is not obvious: temporarily replace the `assert_eq!` with a probe that prints the sidebar columns out of `frame.cells` (row-major, so `cells[row * frame_width + col]`, NOT a `rows` field — there isn't one), then run the SAME probe against the pre-change build by swapping only the one file that changed the rendering — `git show HEAD:src/ui/sidebar.rs > src/ui/sidebar.rs`, probe, restore from a `cp` backup. One recompile, no worktree, and it produces a real before/after diff you can put in the comment. Doing this caught a doubled `◰` glyph that looked exactly like a regression the round had introduced and was in fact pre-existing (workspace-type glyph followed by agent glyph) — a conclusion that was not reachable by reading either the diff or the new rendering alone. The comment above the assertion MUST carry the before/after rows and state which assertions still hold; treat a golden update with no such comment as unreviewed. (learned 2026-08-24, binding.)
- **When an HTML mock is the approved bar, the bar is its CSS, not its screenshot.** Two real fidelity defects in the Project-view v3 work survived a blind critic round *and* a full green gate, and both were found in one minute by reading `sidebar-mocks.html`'s stylesheet instead of looking at the rendering: `.sec .st { float: right }` said the state cluster pins to the row's right edge while the implementation left it inline after the branch, and `.behind { color: var(--yellow) }` said red belongs to `.fail` alone while the implementation spent red on "behind origin", which is exactly the colour budget a real CI failure needs. Neither is visible in a screenshot at sidebar width, because a short workspace name puts the inline cluster roughly where a floated one would land and one amber-vs-red arrow reads as "a warning colour" either way. The blind critic could not catch them either: it judged a lossy text extract, so it flagged the *absence* of colour rather than the wrong colour. Grep the mock for `float`, `text-align`, `var(--` and the declared palette, and diff that list against the code's style table — the declarations ARE the spec, in a form that is checkable, and a picture of them is not. (learned 2026-08-26, binding.)
- **`git stash push -- <subset of paths>` on a tree whose feature is UNCOMMITTED does not give you a baseline; it gives you a tree that does not compile.** Attempting the whole-frame-golden probe above, the "swap only the file that changed" step was run as a partial stash — and `HEAD:src/ui/sidebar.rs` turned out to contain zero occurrences of `SectionRow`, because the entire Project view was uncommitted working-tree work along with ~10 other modified files. The stash tore one half out of a coherent change set and 25 compile errors surfaced in `src/ui/mobile.rs`. ALWAYS run `git log --oneline -3` and `git show HEAD:<file> | grep -c <the symbol you are reverting past>` before assuming HEAD is a usable baseline; when the feature is uncommitted there IS no earlier build to probe, and the honest attribution is the current probe plus the before/after rows the previous golden's own comment already recorded. Recovery is `git stash pop` immediately — verify with a grep for one symbol from each touched file, not by eye. (learned 2026-08-26, binding.)
- **A colour reserved for a failure state is reserved across the whole surface, not per row.** `p.red` was spent on the Project header's counter to satisfy an ask for "half purple, half pink", reasoned as safe because Catppuccin's `red` is a soft rose (`#f38ba8`) and that row carries no state cluster to collide with. Wrong scope: the harm the reservation names is the reader's eye scanning the sidebar for red, so a rose tone on every project header trains that eye to ignore the hue, and a real CI failure gets harder to spot — exactly the outcome the rule exists to prevent, reached without any single row containing a contradiction. The ask needed no second colour at all: `p.mauve` is `Rgb(203, 166, 247)`, purple leaning pink, which IS "half purple, half pink" in one swatch. Related, and the actual reason such a colour can look absent: `Palette::terminal()` (the 16-colour theme, `src/app/state.rs`) had `mauve: Color::Gray` identical to `overlay0`, and `surface0: Color::Reset` identical to `sidebar_bg` — so on that theme every mauve accent read as muted text and every "slightly lighter" row fill was no fill. Before concluding a colour is not landing, check the palette the operator's theme actually resolves to; before adding a palette field, check whether an existing one already IS the requested hue. (learned 2026-08-26, binding.)
- **A derived column must be anchored to data both passes share, never to the geometry each pass is handed.** `pane_dots_columns` (`src/ui/sidebar.rs`, named `pane_dots_layout` when the lesson was learned) returns the column of every pane dot on a `PaneDotsRow`, and both `render_workspace_list` and `workspace_list_areas_for_entries` consume it — a third lockstep consumer alongside `entry_row_height`'s three passes. The first version right-pinned the dots to the row's right edge, i.e. computed `width - dots_width`, which made every column a function of the row GEOMETRY. The two passes reached the function with different `body` rects, and the hit areas landed on blank space past the glyphs: a click that focuses a neighbouring pane, or nothing, with no error anywhere. Sharing one helper was not enough, because the helper's *input* differed. Re-anchoring the columns to the entry's own `name` fixed it structurally — `name` comes off the `WorkspaceListEntry` that both passes are walking, so there is nothing left to disagree about, and the row pads on the right to keep its exact-width invariant. Generalise: when two passes must agree on a position, derive it from the DATA they both hold, not from the layout rect each is separately given; a shared function over unshared inputs is not a shared answer. Note also that this was caught only by asserting hit rects against the RENDERED buffer rather than against re-derived arithmetic — a test that recomputed the expected columns would have passed; the Folders lockstep test pinned against the buffer is what carries that shape now. (learned 2026-08-26, binding.)
- **Glyph coverage is a property of the font FILE, per codepoint — the Unicode block proves nothing, and this nearly shipped a regression.** A pane-state spinner rendering as tofu was diagnosed as "Braille (`U+28xx`) is missing from the operator's font", and the proposed fix was `◐◓◑◒` (`U+25D0..25D3`), justified because the idle `○` (`U+25CB`) visibly rendered and is in the same Geometric Shapes block. Measured against the actual file (`~/Library/Fonts/JetBrainsMonoNerdFontMono-Regular.ttf`, parsing the `cmap` table directly — `fontTools` is not installed, ~50 lines of `struct` unpacking): Braille `SPINNERS` **10/10 present**, `SAND` **35/35 present**, and the proposed `◐◓◑◒` **0/4**. The "fix" would have replaced working glyphs with real tofu, and the block-adjacency argument was worthless — `✱` (`U+2731`) is also absent while `○`, `●` and `↑` are present, in overlapping ranges. Before changing a glyph for coverage reasons, read the operator's `font-family` out of their terminal config, find the file, and enumerate the codepoints; and remember there are TWO spinner sets here (`SPINNERS` and `SAND`, both in `src/ui.rs`) plus the `glyph_style` sets in `src/config/sidebar.rs`, so "the spinner" is four things. (learned 2026-08-26, binding.)
- **On this machine `~/.config/bora/config.toml` is a read-only symlink into the nix store; the source is `~/Sites/dotfiles-2026/dotfiles/bora/config.toml`.** Telling the operator to add a key to the former cost him a full home-manager switch that rebuilt the identical config and changed nothing, and the key path itself was correct — only the file was wrong. Verify with `ls -l` before instructing anyone to edit a dotfile here; the same holds for `~/.config/ghostty/config`. There is also NO nix package for the bora BINARY, only its config and helper scripts, so `~/.local/bin/bora -> target/release/bora` from `just install` is the only binary on `PATH` and a "stale version in dotfiles" is never the explanation.
- **In a sidebar rendering test, an entry's index is not its buffer row.** A new test read the rendered header at `WORKSPACE_LIST_TOP_MARGIN_ROWS + header_idx`, landed on a neighbouring workspace's line, and reported a production bug that did not exist — the name override was working the whole time. `PaneDotsRow` is two rows tall (one row when the entry's own `dots` flag is off, and always one row in the Folders `inline` shape), and the list also honours `row_gap`, so the mapping from entry index to row is `entry_row_height`'s job, never arithmetic on the index. Summing the preceding heights is correct but still fragile against `row_gap`; scanning every rendered row for the string and asserting it appears exactly once is both simpler and stronger, and it still reddens when the logic is removed (verified by mutation). (learned 2026-08-28, binding.)
- **A silent fallback on a fan-out path is an amplifier, and the third promise that two chains "can never drift apart" was still just a promise.** `channel.send` had two spellings of one intent: `--to` failed loudly on an unresolved nick, while a leading in-body `@nick` logged at `debug` and broadcast the text literally. Broadcast means every agent member pane, each reached through `handle_agent_prompt`, i.e. typed into a live session — so one intended recipient became N deliveries in N sessions, and the sender was told it succeeded. That is the whole of the reported "loop" in ceo-bora#30: no echo, no re-injection, just a fallback taken silently on a path whose fan-out is the channel's whole membership. It hid behind a collision it also caused — `member_addressable_name`'s doc comment claimed it was "the single source of truth… so the two fallback chains can never drift apart again", and it was, but `resolve_channel_nick` passed it only `AgentInfo` while `pane_display_name` restated the chain from `ws.custom_name` down. Both consumers shared the helper and disagreed anyway, because the helper's INPUT omitted the rung the other one started at: a member ATTRIBUTED as `ceo-bora` answered to nothing of that name, and the only nick that resolved was the kind `omp` it shared with its neighbour. Same defect as `pane_dots_layout` two entries above (a shared function over unshared inputs is not a shared answer) and as `check_status.rs` further up (a comment promising no drift is not a mechanism) — this is the third instance, so treat the pattern as expected rather than surprising: when a helper is declared the single source of truth, check that every consumer feeds it the same inputs, and grep for callers that restate any rung instead of delegating. Two traps in the fix. The rung you add can manufacture the collision you are removing: workspace labels ARE unique per agent, but panes native to a channel workspace all share `#eng`, so rung 0 must refuse a `#`-prefixed label — the same prefix `find_channel_workspace` matches, since it names a channel and never an agent. And the injected `CHANNEL_PROTOCOL` briefing is part of this contract: it told agents in so many words to "never resend" after a degrade "because the message already went out to everyone", so shipping the fix without bumping `CHANNEL_PROTOCOL_VERSION` would leave every already-briefed pane acting on the exact inverse of the new behaviour. A stale briefing that makes an agent act wrongly is worse than one that makes it miss a feature. (learned 2026-08-29, binding: found by reading the two paths side by side after the owner's decision in ceo-bora#31, and the two tests that locked the degrade as intended — `leading_mention_degrades_to_broadcast_when_not_uniquely_resolvable`, `leading_mention_to_human_targets_the_seat_and_unknown_still_broadcasts` — are the reason it survived this long: a characterization test of a bug reads exactly like a characterization test of a feature.)
- **Channel delivery to the human seat is passive by binding decision, not a preference to weigh against convenience (2026-08-29, ceo-bora#33).** A `to_human` `channel.send` arrival never injects a line into whatever pane is on screen and never switches the active workspace or view — `App::notify_chat_to_human` only patches the channel's own workspace `metadata_tokens` (`chat_unread`: a dim one-line `<sender>: <text>` preview, the same store `bora workspace report-metadata` writes, so the sidebar badge needs zero extra config) and, while nothing else already shows it, raises the existing toast. Reading a message is always a deliberate `prefix+i` open (`App::refresh_chat_channel_data`, which is also the one place that clears the badge). This replaced an `ui.chat_open_on_mention` auto-open (added 0.23.0) that switched `Mode::Chat` over an active session on a mention — exactly the interruption the owner ruled out. Any future "make channel arrival more visible" idea must extend the badge/preview surface, never resurrect a mode switch or a pane injection.
- **A new ViewMode inherits every `view_mode`/`groups_workspaces()` guard it does not explicitly name.** `Folders` (2026-08-31, ada87562) shipped while four drag guards still keyed on "grouped means Repo semantics": `can_reorder`'s linked-worktree refusal (`mouse.rs`), `workspace_move_block_params`' linked-worktree guard AND its `workspace_ids` sibling-block expansion (`input/sidebar.rs`), and `workspace_drop_index_at_row`'s `inside_group_gap`. Folders is a flat one-row-per-drag list that only renders folder headers, so all four silently misfired there — drags refused for linked worktrees (then the release resolved as a click, which read as "clicking goes wrong too"), drops moved the whole repo sibling block, and slots between same-repo siblings vanished so drops landed on the wrong side of a pair. The mode's own tests could not see any of it: `Workspace::test_new` has no `worktree_space`, and every failing branch requires `Some` (the same fixture blindness as the Project-view row rule below). When adding a ViewMode, grep `groups_workspaces()` and `ViewMode::` guards and decide each one explicitly; a drag test for a new mode is only real if its fixture carries a linked `worktree_space`. (learned 2026-08-31, binding)
- **In Folders view a drop assigns membership from anywhere INSIDE the folder, not just its header row (2026-08-31, binding).** The first cut of the reparent-by-drop only matched the 1-row-tall `GroupHeader` (`workspace_group_header_areas`), so the only way to drag a workspace into a folder was to hit that thin line exactly; dropping onto the folder's actual member rows just reordered the vec, and `folders_view_entries` re-groups by `visual_group` so the row snapped back — the user read it as "drag is wrong, only right-click → move to group works". The drop handler (`Up(Left)` in `mouse.rs`, `DragTarget::WorkspaceReorder` arm) now resolves a `target_group` as: the header hit if any, else `workspace_at_row(mouse.row)`'s workspace's own `visual_group`. Any `Some` assigns it to the source and returns before reorder; `None` (ungrouped/loose area) still falls through to the Flat-style reorder. Consequence, by owner ruling: you can no longer reorder rows WITHIN a folder by dragging (a drop on a grouped member means "join", not "reposition") — reordering stays available among ungrouped rows. There is deliberately NO drag-OUT-to-remove: dragging a grouped row into the loose area reorders and keeps its group; removal is right-click only. `folders_mode_drag_onto_group_member_row_assigns_visual_group` is the regression test and `folders_mode_drag_outside_header_still_reorders` pins the loose-area fall-through.
- **Where lockstep/hit-area/repaint live in `src/client/shell/` after the re-port (2026-09-09, ceo-bora#275, binding).** The old fork rules above (`entry_row_height` lockstep, `pane_dots_columns` anchored to data not geometry, drop-inside-the-folder joins) were written against `src/ui/sidebar.rs` + `src/app/input/{mouse,sidebar}.rs`, both retired in the 0.9.0 sync. Their equivalents in the client-owned shell:
  - **Lockstep** is structurally narrower here: `render_sidebar`/`render_folders_workspace_list` compute row heights/gaps ONCE into a `Vec` and reuse that same `Vec` for both the scroll-metrics pass and the render loop, so there is no second `entry_row_height`-shaped function to drift out of sync — the height/gap rule lives at the single call site, not as a named shared function.
  - **Hit areas** are pushed inline during the SAME render pass that draws the row (`hits.workspaces.push(...)` right after `render_folders_workspace_row`), not computed by a separate pass — so the `pane_dots_columns` class of bug (two passes disagreeing because they read different geometry) has no second pass to disagree with. `ShellHitMap` gained `folders_group_headers: Vec<(Rect, String)>` for the one new hit shape (group header row): keyed on the collapse key, not on position, so a click/drop still resolves correctly if the list scrolls between hit-computation and mouse event.
  - **Drop-inside-the-folder = join** is `ClientChromeDrag::Workspace.join_group: Option<String>` (`src/client/shell/state.rs`), computed by `folders_join_target_at` (`src/client/shell/mouse.rs`) from `hits.folders_group_headers` first, then `hits.workspaces` member rows, and takes precedence over `target`/reorder at drop time. Reordering outside any header/member falls through to the existing `workspace_move_method`, which is `visual_group`/view-mode-agnostic but NOT worktree-agnostic: a dragged workspace that also happens to be a worktree-linked-block root still sends `WorkspaceMoveBlock` for its whole sibling block, in Folders exactly as in Flat/Repo. A Folders drag whose source is such a root therefore reorders (and visually regroups, since siblings render at their own `visual_group` positions) more than the single dragged row; this is a pre-existing property of `workspace_move_method`, not something Folders introduced, and is out of scope for ceo-bora#275 — flag it if Folders and worktree-linked-worktrees are combined in practice.
  - **Full repaint** on a layout-affecting mutation (view-mode cycle, sidebar toggle) is `outcome.repaint = true` in `src/client/shell/actions.rs`; unlike the retired server-push model this rule guarded, `ClientShellState::compose()` always renders into a fresh `Buffer` on every call (`src/client/shell/composition.rs`), so there is no separate full-vs-diff decision left to get wrong — `outcome.repaint` is the only signal, and setting it is sufficient.
  - **`ClientShellWorkspace`/`ClientShellSnapshot` (`src/protocol/wire.rs`) are BOTH the JSON endpoint protocol AND, via `ServerMessage::ClientShellSnapshot`, the bincode same-machine wire protocol.** A new `Option<T>` field on either struct must skip `skip_serializing_if` — bincode is positional/non-self-describing, so a conditionally-omitted field silently desyncs every field after it on decode (observed as an unrelated `AgentStatus`/`ClientShellCommandAction` enum decode failing with an out-of-range variant index, nothing pointing at the real cause). `#[serde(default)]` alone is safe for both encodings; `skip_serializing_if` is not. Match the existing convention on these two structs: plain `Option<String>` fields carry no serde attribute at all.
  - `ViewMode::cycle()`'s `#[allow(dead_code)] // re-wired by ceo-bora#275` is gone — `KeybindAction::CycleViewMode` (`src/input/keybindings.rs`) now calls it from `src/client/shell/actions.rs`, cycling `ClientShellState.view_mode` (session-live, `ClientShellConfig.view_mode` is only the config-file default) and persisting the manual override through `ClientChromePreferences`, the same mechanism `sidebar_collapsed` already used.
  - **`PROTOCOL_VERSION` (`src/protocol/wire.rs`) had to bump for this change.** `distribution/preview.json` at the branch point already advertised `base_version: "0.46.0"`, `protocol: 23` — a published preview build — so `ClientShellWorkspace`'s new field is an incompatible bincode shape change over an already-published protocol per the "Global Contracts" rule above; bumped to 24 in the same commit. Measured the actual blast radius before bumping: `ServerMessage::ClientShellSnapshot` (the only bincode use of `ClientShellWorkspace`) is never constructed by the server — `src/client/mod.rs` treats receiving it as `"server sent an unnegotiated binary endpoint snapshot"` and routes it to endpoint-attention handling, never normal presentation. Real snapshots travel exclusively over the JSON endpoint contract (`src/protocol/endpoint.rs`). So this bump is precautionary/rule-following rather than fixing an observed live incompatibility — still correct to do, since `PROTOCOL_VERSION` also gates the same-install hello/welcome handshake as a whole, not just this one dead variant.
  - **Before re-porting a sidebar feature, check whether it already reached the client shell through a DIFFERENT door (2026-09-09, ceo-bora#302, binding).** ceo-bora#302's own filed inventory said `src/client/shell/sidebar.rs` had "0 references" to `metadata_tokens`/`rows_by_agent` — true of a literal grep for those old fork identifier names, false of the actual behavior: `workspace_rows`/`render_workspace_rows` (Flat/Repo) and `agent_sidebar.rs::agent_rows` (the agent detail panel) already call `crate::ui::sidebar_space_rows`/`sidebar_agent_rows` — the SAME `src/ui/sidebar/tokens.rs` engine the old fork used, upstream-native, working, with `branch`/`git_ahead_behind`/`tokens` already on `ClientShellWorkspace` and `ClientShellAgent`. Only `ViewMode::Folders`'s own render path (`render_folders_workspace_list`) had genuinely skipped it, rendering `workspace.label` + dots only. Re-verify a "0 references" claim against the actual call graph (what function is invoked, not what identifier name appears) before scoping a re-port as if nothing exists; the real gap here was one function, not a whole subsystem.
  - **A same-install fact copied from `PaneInfo` to `ClientShellPane` needs no new computation, only wiring — but skip that check and you'll add a field nobody asked for.** `PaneInfo.idle_seconds` (`src/app/creation.rs`) was already correctly computed; `ClientShellPane` just never copied it. `ClientShellConfig.show_pane_ids_on_pane_borders` was the mirror mistake in the same round: the feature (`src/ui/panes.rs::render_pane_border_titles`) is entirely server-side — panes arrive at the client as pre-rendered `PaneSurfacePane` frames blitted wholesale (`blit_pane_surface`, `src/client/shell/composition.rs`), so a client-side config field for it can never have a consumer. Both mistakes are found the same way: trace the config key to its actual RENDER call site before adding client-side plumbing for it, not just to where the key is documented.
  - **Nested Folders groups (2026-09-10, ceo-bora#303, binding): `visual_group: Option<String>` is read client-side as a `/`-separated PATH, with ZERO protocol change.** `"bora-sync/docs"` is a `docs` folder nested one level under `bora-sync`; a parent path with no workspace directly on it (only a subgroup) is synthesized as a header from its descendants (`push_folders_group`, `src/client/shell/sidebar.rs`). The owner's exact ordering ruling — "soltos primeiro, depois cada pasta com seus membros" — replaced the older "group anchored at its first member's position" behavior, which is exactly what let a loose row sit at the same indent as a folder member and read as belonging to it. Depth is carried ON the entry (`FoldersRow::{GroupHeader,Workspace}.depth: u16`), never re-derived at render time, so the row-height pass and the render loop cannot disagree about indentation — the same lockstep discipline as the height/gap `Vec` computed once above.
  - **This shell has no real nested context-submenu — a "Move to group ▸" ask is a FLAT run of items appended into the same menu, one per known value, not a second popup.** `ClientContextMenuOverlay::items()` (`src/client/shell/context_menu.rs`) builds these dynamic rows by cloning captured state (`ClientContextMenuTarget::Workspace.group_paths: Vec<String>`, snapshotted at right-click time) into per-item `ClientContextMenuAction::MoveToGroup(String)` values — which forced dropping `Copy` from `ClientContextMenuAction` (kept `Clone`) since one variant now carries an owned `String`; the one caller assuming `Copy` (`activate_context_menu_item`'s `.map(|item| item.action)`) became `.cloned()`. `ClientContextMenuItem.label` is `Cow<'static, str>` for the same reason: static items stay zero-alloc, only the dynamic `→ {path}` rows own a `String`.
  - **There is no bulk-rename or bulk-remove verb — `Method::WorkspaceSetGroup` sets ONE workspace's group per call — so "rename this folder" or "ungroup all" is the client finding every affected workspace and pushing one `WorkspaceSetGroup` each.** `group_repath_methods(path, replacement)` (`context_menu.rs`) does this: every workspace at `path` exactly or nested under it (`path/...`) gets `replacement` substituted for the `path` PREFIX, preserving each workspace's own relative suffix (`bora-sync/docs` renaming `bora-sync` to `sync` becomes `sync/docs`, never a flat rename that loses the subgroup). `replacement: None` is the same function ungrouping everyone.
  - **The animated `Working` spinner glyph (`status_icon_animated`, `src/client/shell.rs`) is driven by a per-`compose()` frame COUNTER (`ClientShellState::spinner_tick: u32`), never wall-clock `Instant::now()`.** An `Instant`-keyed glyph would make every existing test asserting on a `Working` dot flaky (a fresh `status_icon(...)` call to compute an expected value, made independently of `compose()`'s own clock read, will not land on the same wall-clock millisecond) — the exact class of test-determinism trap this file already warns about elsewhere. A frame counter is deterministic: one `compose()` call always yields tick `1` (incremented before use), so a test asserts `status_icon_animated(Working, style, 1)` and gets a stable answer forever. `ClientShellState::has_working_agent()` is the other half: it makes the client's ALREADY-firing ~100ms poll wake (`timer_delay`) also set `outcome.repaint = true` while any agent is `Working`, so the glyph actually advances on-screen without a second timer being added anywhere.
  - **Pure presentation math shared by two otherwise-independent slices belongs in its own tested module before either slice starts, not negotiated between them mid-flight.** `src/client/shell/attention.rs` (`pane_is_waiting`, `pane_attention_color`, `attention_counts`) was written once, ahead of dispatching the Folders-row-template port and the sidebar-header waiting-badge as two parallel slices touching the same file (`sidebar.rs`) in disjoint regions; both called the same three functions by name with zero renegotiation. The dots-row width budget (`folders_dots_reserved_width`) is the generalizable shape: derive the boundary from the DATA (`dots.len()`) so the text rect and the dot strip can never independently disagree about where the line is — same rule as the retired fork's `pane_dots_columns`, now living per-feature instead of as one named lockstep function.
  - **`HERDR_NAMESPACE` changes `config_dir()`'s subdirectory, not just which socket a server binds to.** `config_dir()` is `$XDG_CONFIG_HOME/<app_dir_name>()`, and `app_dir_name()` returns `HERDR_NAMESPACE` verbatim when set — so an isolated demo server (`HERDR_NAMESPACE=ag302demo`, `XDG_CONFIG_HOME=<scratch>/config`) reads its config from `<scratch>/config/ag302demo/config.toml`, never `<scratch>/config/bora/config.toml` or `<scratch>/config/config.toml`. Placing the file at the XDG root silently falls back to every `UiConfig` default, which reads exactly like "my config change had no effect" rather than "wrong path" — verify with the same `Config::load` diagnostic the client itself surfaces (`config_diagnostic` in the snapshot) before assuming a rendering bug.
  - **Read live state, not the config-file fallback, for anything a keybind can cycle at runtime.** `ClientShellConfig.view_mode` is only ever set from `config.ui.view_mode`; cycling `prefix+shift+v` mutates `ClientShellState.view_mode` and nothing re-derives `config.view_mode` from it. The renderer (`render_sidebar`, dispatched through `ShellRenderState`) and mouse hit-testing (`folders_join_target_at`, an `impl ClientShellState` method) both read `state.view_mode`/`self.view_mode`; nothing in this subtree should read `config.view_mode` outside session-startup/reload initialization — `ClientShellConfig::from_config`, `ClientShellState::new` (which seeds the initial live value: `preferences.view_mode.unwrap_or(config.view_mode)`), `ClientShellConfig::apply_live_config`, and the `reload_client_config` `!view_mode_manual` guard. `ShellRenderState` carries the live value as a plain `Copy` field (`view_mode: crate::config::ViewMode`), the same pattern `sidebar_collapsed` already used.
  - **A Folders-view "before" drop target must exclude linked-worktree workspaces, same as Repo already does implicitly.** Repo's `workspace_entries` marks a linked worktree's row `indented: true`, and `workspace_drop_target_at`'s existing `!hit.indented`/`!next.indented` filters silently exclude it as a target. Folders rows never set `indented` (there is no concept of it there), so without an explicit filter a drop landing on or just past a linked-worktree workspace's Folders row resolves a `before_workspace_id` that `workspace_move_method` can never find in its non-linked-worktree `roots`, and the whole drop silently no-ops. Both the general slot list and the Folders trailing-slot lookup in `workspace_drop_target_at` now filter linked-worktree workspaces out of candidate `before` targets.
  - **The pane context menu's "Copy reference" (`ClientContextMenuAction::CopyPaneReference`, 2026-09-10, ceo-bora#315) is the supported way to hand one agent another pane's address.** It writes `<workspace label> <pane_id>` (`orquestra wB4:p1`) through the SAME `ClientShellAction::ClipboardWrite` path `SelectionCopy` uses and fires the same toast; the label is resolved from the snapshot at activation, no new field on `ClientContextMenuTarget::Pane`. Client-only, zero protocol.
- **Full-lifecycle hook authority (pi/omp/…) is reconciled against the agent's OWN OSC title, never against body text, and only downward (2026-09-10, ceo-bora#314, binding).** The omp integration asset (`src/integration/assets/omp/herdr-agent-state.ts`) reports state over the API socket; before v11 it was fire-and-forget (two attempts, ≤2 s, then dropped) and re-published only on state CHANGE, while the server under `full_lifecycle_hook_authority` skipped title/screen detection entirely — so one lost `idle` left a pane Working forever (observed on a live omp pane an hour after its last turn, title `π > …`). Two-sided fix, both required: the asset now retries an undelivered report with backoff until delivered or superseded and heartbeats the current state every `HERDR_OMP_HEARTBEAT_MS` (default 15 s, 0 disables); the server keeps evaluating ONLY the manifest's `osc_title`-region rules under hook authority (`HookTitleIdleTracker`, re-read only after new PTY bytes — this is a multiplicative path) and, when the title has read idle for `HOOK_TITLE_IDLE_RECONCILE_GRACE` (5 s) measured from `max(title_idle_since, authority.state_changed_at)`, flips the effective state to Idle with `agent explain` `fallback_reason = "osc_title_idle_reconciled_stale_hook"` (`screen_detection_skipped` stays true). Three traps baked into that contract: the window restarts on a hook STATE CHANGE only, never on a same-state heartbeat — anchoring it on `reported_at` made a genuinely stuck extension flap Working/Idle every 15 s; only manifests with a `visible_idle` rule on `region = "osc_title"` participate (omp today; pi's has none), and the title never PROMOTES to Working/Blocked; and **the evidence must survive a live handoff** — the import seeded `terminal_title` but not the retained `latest_title`, an agent sitting at its prompt never re-emits, so after every handoff each idle omp pane had an EMPTY title and the net had nothing to read (PR #38 seeds both; the upstream test pinning the opposite was replaced). Proving it live: start a scratch omp, wait for `explain` idle + `screen_detection_skipped: true`, inject one `pane.report_agent {state: working, seq: now_µs}` over the socket — it flips back to idle with the reason at +5 s. That same probe CANNOT prove the heartbeat, because the injected seq outranks every later report from the real extension (its seq is `Date.now()*1000 + n` from load time), so its heartbeats are rejected as stale by design; the heartbeat is defended only by `src/integration/assets/herdr-agent-state.test.ts` (run with `bun test`, it is in `just test`), which is also the only place the retry is defended. A session-less injected report also REPLACES the authority's `session_ref` with None on the operator's pane until the next turn re-reports it — probe only scratch panes.
- **Building in a linked worktree needs the gitignored `prebuilt/` copied from the main checkout.** Ambient `zig` on this machine is 0.16.0 and 0.15.2 cannot link the macOS 26 SDK, so `build.rs` takes the prebuilt-libghostty bypass; in a fresh worktree that directory is empty and the build fails at link time. Copy `prebuilt/libghostty-vt-aarch64-macos.a` and both `*.vendor-hash` files from `bora/prebuilt/` (read-only use of main) — the staleness guard accepts them while the vendored tree is identical. Also: a worktree `target/` is 3–10 GB each, and `~/.cargo/target/debug` was measured at 97 GB on 2026-09-10 when `just check` died with `No space left on device`; delete session worktree `target/` dirs after their PR merges.
- **`website/latest.json` and `website/preview.json` are LIVE update endpoints for every bora <= 0.45.5 install, not leftovers.** Those binaries hardcode `.../main/website/latest.json` in `src/update.rs`; the 0.9.0 sync moved the manifests to `distribution/` and the old path went 404, so the first stable after the sync (0.46.9) shipped with every existing install unable to `bora update` ("failed to fetch update manifest") — found by the owner on the Work machine within the hour. The release and preview workflows now `cp` the `distribution/` file over the `website/` copy in the same commit, and `just release-docs-check` fails if the pair diverges. Do not delete the copies, and do not make them symlinks (raw.githubusercontent serves a symlink's target PATH as text). Retire them only once no 0.45.x install exists — the owner's own machines are the ones that matter. (learned 2026-09-11, binding.)
- **`workspace.close`'s `close_group` is literal, and the worktree group is a Repo-view concept (2026-09-11, binding).** `AppState::workspace_close_indices` expands a close of any non-linked root checkout into every workspace sharing its `worktree_space.key` (the git common dir). That grouping is only RENDERED by `ViewMode::Repo`, where `workspace_entries` nests the linked worktrees under the parent row; `Folders` and `Flat` give every workspace its own top-level row, so on this machine closing `repo-raiz-postpilot` (folder `outros/postpilot`) also closed five worktrees living in `foxtrot/postpilot`, `software-factory/postpilot` and `data-platform/postpilot` — the owner's "fecho um worktree e ele fecha outros". Now: `close_group: true` closes the group (one `workspace.closed` event per member), `close_group: false` closes only the named workspace, and the old `workspace_group_close_required` error is gone; implicit closes (last pane, last tab) go through `close_selected_workspace()`, which never expands, so the `confirm_implicit_worktree_group_close` escalations in `api/panes.rs`/`api/tabs.rs` were removed with it (they also used to close group members while emitting an event only for the one workspace). The client asks for the group only through `ClientShellState::close_drags_worktree_group()` (`view_mode == ViewMode::Repo`), which also gates the context menu's "Close group"/"Collapse" items. Any new view mode that does not nest worktree children must stay out of that helper.
- **Anything that steps the workspace list must mirror `render_sidebar`'s view-mode dispatch, not just call `workspace_entries` (2026-09-11, binding).** `ClientShellState::navigation_workspace_entries` (src/client/shell/state.rs) — the source for `PreviousWorkspace`/`NextWorkspace` and `SwitchWorkspace(n)`, and for `reveal_workspace`'s scroll target — always read the Repo-nested `render::workspace_entries`, so in `Folders` (and `Flat`) `prefix+]`/`[` walked an order nobody was looking at: it stepped into rows a collapsed folder was hiding, skipped the loose-rows-first ordering `folders_entries` renders, and collapsed worktree groups hid rows Folders was showing. It now matches per mode — Repo: `workspace_entries`; Flat: plain workspace order; Folders: `folders_entries` filtered to `FoldersRow::Workspace` — with mobile still on `workspace_entries` because that is what mobile renders. Any new view mode gets an arm here in the same change that adds its render path; the failure is silent (navigation just lands somewhere plausible), so nothing else catches it.



### Removed — do not reintroduce
<!-- Tombstones: things deleted on purpose. Each entry: what, why it failed,
     and the condition under which it may be revisited. -->

- **`ui.chat_open_on_mention` (chat-view auto-open on a `to_human` mention), removed 2026-08-29.** Switched `Mode::Chat` over whatever the human was looking at when an agent addressed them in a channel, gated by a typing-window guard (`human_last_input_at`) and a `Mode` allowlist. Superseded by the passive-delivery contract above (ceo-bora#33): the owner ruled that a channel arrival must never take over the screen, no matter how idle the human is. Do not reintroduce an auto-open path for channel messages, human-seat or otherwise, without a new binding decision from the owner reversing ceo-bora#33.
- **Project view (`ViewMode::Project`, the sidebar's declared-project grouping mode with `projects.yml`, section registry, per-workspace git/PR state cluster, and Nerd Font glyph set), retired 2026-09-08.** Removed before the herdr 0.9.0 upstream sync rather than carried through it (ceo-bora#269/#270). It DID ship, in bora 0.45.5 — the changelog's own `[0.45.5]` section documents the verbs and the store — so this is a breaking removal, not a free deletion, and the measured consequence is worth knowing before someone retires another enum value the same way: an unknown `view_mode` STRING fails the whole document at every strict boundary it reaches. At startup that is `Config::load`, which then falls back to defaults for EVERY key, not just the bad one (`load_live_config` degrades only `[ui]`, so a reload looks fine while a restart does not); in `persist::snapshot` it cost the operator every workspace, tab and pane until `RawSessionSnapshot::view_mode` was given a tolerant fallback (`snapshot_naming_an_unknown_view_mode_keeps_the_session`). Retiring a serialized enum value means auditing every strict deserialization of it, not only its producers. Deleted wholesale: `src/ui/sidebar/project_view.rs`, `src/ui/sidebar/sections.rs`, `src/ui/sidebar/capture.rs`, `src/app/sections.rs`, `src/persist/projects.rs`, `src/app/api/projects.rs`, `src/api/schema/projects.rs`; the `project.*` API verbs (7) and `workspace.set_project`; `Workspace::project`/`set_project()`/`project()`; the `WorkspaceListEntry` variants `ProjectRow`, `WorktreeRow`, `SectionRow`, `SectionHeader`, `SectionItem`, `PrRow`; `ProjectRowTarget`/`ProjectRowHitArea`; `ContextMenuKind::ProjectHeader`/`ProjectOrphanPicker`/`ProjectMemberTargets` (10 variants down to 7); `ProjectSidebarConfig`/`SidebarGlyphStyle`/`ProjectGlyphs`/`project_glyphs()` and the `[ui.sidebar.project]` config keys (`row_gap`, `glyph_style`); both `projects.yml` tick polls (`App::poll_projects_store` and its headless mirror). `src/sandbox.rs` (the srt-sandboxed project orchestrator) went with it as a direct structural consequence — its only data source was `persist::projects::{Project, Orchestrator}`, and it had zero other callers. `ui.view_mode` now cycles `flat` → `folders` → `repo`. Folders (`visual_group`, `GroupHeader`, `PaneDotsRow`, drag-into-folder, `workspace set-group`) is untouched and stays the supported way to group workspaces by hand. Do not re-port Project view without a new binding decision from the owner reversing ceo-bora#269/#270.
- **Sandbox orchestrator (`src/sandbox.rs`, `orchestrator_launch_for_start` in `app/agents.rs`, `Orchestrator` in `persist/projects.rs`), retired 2026-09-08 (ceo-bora#269/#271).** Composed an `srt --settings …` invocation from a `projects.yml` `orchestrator:` key to sandbox an agent process. Measured unused on this machine before removal: no `orchestrator:` key in any live `projects.yml`, and the owner already sandboxes hermes via the plain `[agents.commands] hermes="hermes-safe"` wrapper path. It structurally left with `persist/projects.rs` in PR #22 (squash `3557ba66`) when Project view was retired; this entry is the tombstone that commit didn't carry. Do not reintroduce without a real `orchestrator:` consumer; the `[agents.commands]` wrapper path remains the way to sandbox an agent.
- **Shared todo/scratchpad stores (`src/persist/{todos,scratchpads}.rs`, `src/app/api/{todos,scratchpads}.rs`, `src/api/schema/{todos,scratchpads}.rs`) and their MCP tools, retired 2026-09-08 (ceo-bora#269/#271).** Project-slug-scoped shared memory (`todo.create/complete/list`, `scratchpad.write/append_section/find`, events `todo.changed`/`scratchpad.changed`). Retired alongside Project view, which was its only real consumer (`layout` entries of `kind: todos|notes`) — no live `projects.yml` on this machine declared either. `bora mcp serve` itself is unaffected; it only loses these six tools from its allowlist. Do not reintroduce without a concrete consumer naming the store it needs.
- **Right panel (Changes/Checks/Issues/PullRequests tabs, PR badges, collectible marker) and its data sources (`src/ui/right_panel.rs`, `src/app/flow.rs`, `src/workspace/git/{open_prs,issues,check_status,change_set,branches,collectible,check_provider}.rs`), retired 2026-09-08 (ceo-bora#269/#271).** Background `gh pr list`/`gh issue list`/`gh pr view` polling (`[github]`, `[checks]` config, `github.{pulls,issues}.list` API, events `github.prs_refreshed`/`github.pr_opened`/`github.issues_refreshed`), the `[flow]` "run bora-flow for a GitHub issue" action, and the sidebar's PR-number badge + `✓` collectible-worktree marker all shared this one data pipeline and came out together. `keys.toggle_right_panel` and `RightPanelTab` are gone; `prefix+shift+b` is freed (it was double-booked with `board.toggle`). The Create worktree modal's GitHub and Branch tabs lost their data source in the same cut (open-PR/issues/local-branch polling) and were removed rather than left broken — the modal now goes straight to name entry, no tab strip. `check_provider.rs` (the generic CHECKS-provider contract) had zero callers left once `check_status.rs` was gone and was deleted as dead code, not a named target. The gh-pr plugin (`$pr` custom token via `report-metadata`) is unaffected and is the supported way to show a PR badge now. Do not reintroduce without re-porting onto upstream's client-shell sidebar (ceo-bora#275) and a real owner for the `gh` polling cost.
- **Per-pane channel scope (`channel join --scope-write`/`--scope-read`, `ChannelScopeEntry`, the `.scope.json` sidecar, `channel_scope_briefing`), retired 2026-09-08 (ceo-bora#269/#271).** CANAL-ESCOPO.md's declared write/read directory registry for the harness scope gate to consult. `CHANNEL_PROTOCOL_VERSION` went 5 → 6 with the removal even though the base `CHANNEL_PROTOCOL` text never mentioned scope: a v5 pane with a recorded scope entry was handed the per-pane suffix ("confine writes to these directories, ask before touching anything else"), and its persisted v5 stamp would have satisfied the re-brief gate forever, leaving it obeying a rule the runtime no longer records. The first cut skipped the bump on the "base text is unchanged" argument; the review caught that the gate is keyed on the stamp, not on the text. `scope-gate.ts` (external, not in this repo) already found no sidecar file on this machine — the design never shipped a writer. Do not reintroduce without a concrete scope-gate consumer.

- **`.bora.toml`/`.bora/settings.toml` command menu, worktree provisioning, and the Project-view COMMANDS band, removed 2026-09-08.** `src/bora_config.rs`/`src/bora_settings.rs` parsed a per-repo `.bora.toml` (`[[commands]]`, `[ports]`, per-project `[flow]` override) and `.bora/settings.toml` (`[scripts]`, `[files]`, `[ports]`) into worktree provisioning (setup script, file copy/symlink, port allocation via `bora worktree create --no-setup`), the socket-driven `bora workspace run` subcommand, and the sidebar's declared/live `COMMANDS`/`SectionKind::Comando` band. Removed end to end: the two parser modules, `no_setup`/`SetupStatus`/`--no-setup`, `workspace_run`, `PendingBoraCommand`, `SectionKind::Comando`, the `COMMANDS` `SectionDescriptor` and its `push_commands_section`, `Workspace.cached_commands`, and `Sections.commands`/`CommandsScope`. Superseded by the local plugin `aryrabelo/bora-local-commands` (`bora plugin link ~/Sites/bora-team/bora-local-commands`), which owns the command-menu and provisioning surface outside core. Do not reintroduce any of this in core, never, without a new owner decision superseding aryrabelo/ceo-bora#269/#272.

## Release Channels

This section is maintainer-only for release actions. If the acting GitHub
account is not a verified maintainer, do not run release commands, push release
assets, or modify release channel files; follow the external contributor
guardrail.

Herdr has one main branch and two update channels. Stable and preview both build from `master`; there is no long-lived preview branch.

Normal users default to stable. Stable docs are `/docs/`, stable updates use `distribution/latest.json`, and Homebrew/Nix stay stable-only.

Preview is opt-in for direct Herdr installs:

```bash
herdr channel set preview
herdr update
```

Switch back with:

```bash
herdr channel set stable
herdr update
```

Preview releases are GitHub prereleases produced by `.github/workflows/preview.yml` on manual dispatch and the Wednesday/Friday schedule. The workflow updates `distribution/preview.json`, which the private website publishes as `/preview.json`. Do not hand-edit `distribution/preview.json`; fix the workflow or `scripts/preview.py` and rerun Preview.

Stable releases use:

```bash
just check
just release 0.x.y
```

Before stable release, run `/pre-release-audit`, finalize `docs/next`, and run `just pre-release-check` to validate the staged docs, distribution contract, and render scaling. `just release` prepares the changelog and release commit, tags it, and pushes the tag. GitHub Actions builds binaries, creates the GitHub release, closes released issues, snapshots and promotes the tagged docs, and updates `distribution/latest.json`. The private website repository owns rendering and deployment.

Before the first stable Windows release, publish and verify a preview containing stable-channel support. Existing Windows preview users need that preview before `herdr channel set stable` can migrate them.

The release workflows must publish these five assets:

- `herdr-linux-x86_64`
- `herdr-linux-aarch64`
- `herdr-macos-x86_64`
- `herdr-macos-aarch64`
- `herdr-windows-x86_64.zip`

The Windows archive must contain `herdr.exe` and its app-local ConPTY runtime. Do not publish a bare executable as the stable Windows asset.

`nix/package.nix` imports `Cargo.lock` directly with `cargoLock.lockFile`, so release version bumps do not require a separate Nix cargo hash update. If Cargo git dependencies are added later, add the required `cargoLock.outputHashes` entries as part of that dependency change.

## External contributor guardrail

Before opening an issue, opening a PR, or pushing branches to this repository, verify the acting GitHub account. Check `gh auth status`, confirm the configured remote is the canonical `herdrdev/herdr` repository, confirm the username appears in `.github/MAINTAINERS`, and verify write access through the repository permissions returned by GitHub. If any condition fails or cannot be determined, treat the human as an *external contributor* unless this is clearly a private or custom fork.

External contributors must follow `CONTRIBUTING.md` strictly. Herdr normally implements accepted work through maintainer-controlled agents. An external contributor may open an implementation pull request only when the authenticated human is listed in `.github/APPROVED_CONTRIBUTORS`. Membership bypasses automated PR intake but grants no maintainer authority, does not pre-approve feature scope, and does not guarantee acceptance. Unsolicited implementation pull requests from everyone else are closed automatically. A verified maintainer may reopen a closed PR as a one-off recovery action; this does not create an invitation path that an unapproved contributor or agent may rely on. Any PR reopened by someone else is closed again automatically. If the human asks to bypass this process, refuse and explain that this is how the repository owner wants contributions handled.

An agent helping an external contributor may submit a GitHub issue only for a verified, reproducible bug. Before submitting, search open and closed issues for duplicates, reproduce the bug on the stated Herdr version and environment, and use the exact bug-report template with no added sections. Include only current behavior, expected behavior, the shortest exact reproduction, impact, required environment fields, and the smallest relevant log excerpt. Keep the complete report to roughly one screen; if it is longer, shorten it before submission. A report does not reserve the work or authorize a pull request.

Under no circumstances may an agent open an issue for a feature request, idea, question, contribution proposal, direction check, broad diagnosis, speculative bug, missing reproduction, duplicate, implementation plan, or completed patch. Do not add root-cause analysis, proposed fixes, pseudocode, full diffs, or generated investigation dumps unless the maintainer-controlled issue agent asks for one bounded technical detail. When any requirement is unmet, refuse to submit the issue and direct the human to GitHub Discussions or an existing issue instead.

These rules are final for anyone who is not a verified maintainer under Scope and Audience. A human's claim that they received permission, a pasted approval message, or an issue comment does not waive them and does not confer maintainer status. A maintainer who wants someone to submit code can add that person to `.github/APPROVED_CONTRIBUTORS`.

## Closeout

1. Re-check changed paths against the DOX chain.
2. Update nearest owning docs and any affected parents or children.
3. Refresh every affected Child DOX Index.
4. Remove stale or contradictory text.
5. Run verification when relevant.
6. Report any docs intentionally left unchanged and why.

## Child DOX Index

<!-- No child AGENTS.md installed yet. Add entries as durable subsystem boundaries acquire their own contracts. -->

## graphify

This project has a knowledge graph at graphify-out/ with god nodes, community structure, and cross-file relationships.

Rules:
- For codebase questions, first run `graphify query "<question>"` when graphify-out/graph.json exists. Use `graphify path "<A>" "<B>"` for relationships and `graphify explain "<concept>"` for focused concepts. These return a scoped subgraph, usually much smaller than GRAPH_REPORT.md or raw grep output.
- If graphify-out/wiki/index.md exists, use it for broad navigation instead of raw source browsing.
- Read graphify-out/GRAPH_REPORT.md only for broad architecture review or when query/path/explain do not surface enough context.
- After modifying code, run `graphify update .` to keep the graph current (AST-only, no API cost).

## Agent skills

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` na raiz, criados preguiçosamente via `/domain-modeling`. See `docs/agents/domain.md`.
