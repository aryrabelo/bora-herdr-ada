# DOX framework — bora

- DOX is a self-documenting AGENTS.md hierarchy installed here (OMP-focused).
- Every agent must follow DOX instructions across any edits.

## Project

herdr is a terminal-based agent runtime for coding agents, written in Rust. Core surfaces: `src/app/` (state, actions, input), `src/platform/<os>.rs` (OS-specific behavior), `src/detect/manifests/` (agent detection), `src/protocol/wire.rs` (server/client wire protocol), and the vendored `vendor/libghostty-vt`. Build, test, and validate through `just` recipes (`just test`, `just check`). Stable/preview both build from `master`.

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
`bora` keeps the old binary.** `~/.local/bin/bora` is a symlink to
`target/release/bora`, so it always looks correctly installed no matter how old
that file is — which means a stale binary is invisible from the outside and a
report of "your change did nothing" can be true of the binary and false of the
tree. `just install` builds release, refreshes the symlink, and prints the
version it just installed for exactly that reason: the version string is the
only cheap proof of what is actually on `PATH`. Because the symlink resolves to
a file that Cargo replaces, an already-running `bora` holds the previous inode
and keeps running the old code until it is restarted. When asking someone to
verify a fix, ask for `bora --version` first. (learned 2026-08-25, binding: a
repaint fix was reported as ineffective while the installed binary was ten
minor versions behind the tree, and the same stale binary had earlier been
suspected of *causing* a regression that shipped after it was built.)

**`just check`/`just lint` only lint the host target you run them on and cannot compile target-gated Rust from a macOS box; only CI's `ubuntu-latest` leg lints that code.** Two separate gating shapes hide code from a macOS run, and the second one is easy to miss: whole-file `#![cfg(not(target_os = "macos"))]` test files (`tests/auto_detect.rs`, `tests/cli.rs`), and platform modules excluded by an **outer** `#[cfg(target_os = ...)]` on their `mod` declaration in `src/platform/mod.rs` (`src/platform/linux.rs`, `src/platform/windows.rs`) — including their `#[cfg(test)] mod tests`. A green `just check` on macOS is not proof any of it is clean; `lint` prints a reminder naming all four files, and that reminder is a to-verify list, not noise. (learned 2026-08-13, binding: clippy failures in Linux-only-gated test files reached CI invisibly from a macOS `just check` this way. Reasserted 2026-08-22, binding: it happened again, and worse — `9a2db191` left `std::sync::{Mutex, OnceLock}` unused in `src/platform/linux.rs`'s test module and CI stayed red across three commits, because the reminder only grepped for the whole-file inner attribute and never mentioned the platform modules at all. The `lint` recipe now lists them explicitly. Cross-compiling to verify locally does not work on this machine: the vendored libghostty-vt build script needs zig 0.15.2 and mise resolves 0.16.0, so CI is the only verifier — push the fix and watch the run.)

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
`scripts/changelog.py check-history-sync`. When a rule is ambiguous on a given
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

## Vendored libghostty-vt

`vendor/libghostty-vt.vendor.json` records the upstream source commit currently vendored.

Local patches on top of the vendored source must be tracked in `vendor/libghostty-vt.patches.md` and stored as patch files under `vendor/patches/libghostty-vt/`. Each entry should say why the patch exists, the Herdr issue, upstream PR/discussion, vendored base commit, touched files, verification, and the exact removal condition.

When updating libghostty-vt, check every active patch in `vendor/libghostty-vt.patches.md`. If the new upstream commit contains the fix, remove the local patch and index entry, then rerun the listed verification. If not, reapply the patch on top of the new vendored source.

`just check` runs maintenance tests that verify local libghostty-vt patch files are listed in the index and reverse-apply cleanly against the vendored tree. Do not leave a patch file untracked or an indexed patch unapplied.

## Docs

`skills/herdr/SKILL.md` tracks the latest stable Herdr release because the unversioned `npx skills add herdrdev/herdr --skill herdr -g` command installs it from `master`. Do not update this file in feature or preview work. Review and update it only during stable release preparation, and include the change in the release commit with the `Cargo.toml` version bump. Preview builds keep the latest stable skill.

Unreleased docs live in `docs/next/website/src/content/docs/`. Update those when a user-facing change needs docs before the next release. They are committed drafts but are never production website input. `docs/next/README.md` and `docs/next/CHANGELOG.md` stage root README and changelog changes: **`docs/next/CHANGELOG.md` is the single source of truth for unreleased entries.** Append new entries only there. Root `CHANGELOG.md` stays release-generated: its `## Unreleased` section must stay empty between releases, and `just release-prepare` promotes `docs/next/CHANGELOG.md`'s Unreleased section into a new versioned entry in both files (`scripts/changelog.py prepare --path docs/next/CHANGELOG.md`, then that result is copied over root `CHANGELOG.md`, never the other direction). `just release-docs-check` (and therefore `just pre-release-check`/`just release-prepare`) runs `python3 scripts/changelog.py check-history-sync`, which fails loudly instead of silently overwriting either file when root has direct Unreleased content or when released history has diverged between the two files — reconcile by hand before releasing if it fires. (learned 2026-08-13, binding: an earlier version of `release-prepare` copied root into `docs/next` and destroyed docs/next-only content; do not reintroduce that direction.)

The active preview release docs live in `docs/preview/website/`. Preview CI owns this mutable snapshot and commits it atomically with `website/preview.json`; never edit it manually. Validate it with `node website/scripts/docs-preview.mjs check`.

Published stable-release documentation lives in `docs/versions/`. Release CI seeds each version from the tagged `docs/next` tree, and maintainers may correct factual documentation errors in a published version afterward. Apply a correction separately to `docs/next` when it also applies to future releases; never replace a published tree with the current draft. The website build generates `/docs/preview/` from the active preview snapshot, `/docs/<version>/` from the maintained version directories, and `/docs/` from the version selected by `docs/versions/manifest.json`. Do not edit generated files under `website/src/content/docs/`.

During release review, finalize `docs/next` and run `just release-docs-check`. Do not copy draft docs into preview or published versions manually. Preview CI snapshots the selected commit. After a stable GitHub Release succeeds, release CI seeds a new version from the exact tag, updates `latest.json`, and deploys them together. Normal feature/fix work should not edit root `README.md`, root `CHANGELOG.md`, published version docs, or `website/latest.json` unless it is a focused correction to already-published documentation or explicitly requested. `docs/next/CHANGELOG.md` is for user-facing Herdr runtime changes; do not add entries for website-only, documentation-only, CI, build-pipeline, or repository-maintenance changes.

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
- Adding an `EventKind` is two lists, not one. A new variant reaches `events.subscribe` as soon as it is in `EventKind` with a `Subscription` arm, but a plugin manifest `[[events]] on = "..."` hook stays inert until the variant is also in `PLUGIN_HOOK_EVENT_KINDS` (`src/api/schema/events.rs`), because `run_plugin_event_hooks` returns early on any kind missing from that list (`src/app/api/plugins/runtime.rs:220`). The failure mode is a silent no-op: the manifest parses, the plugin installs, the hook never fires, and nothing logs. Give every new kind its own arm in the plugin-context match too — one that resolves a real workspace instead of falling into `empty_plugin_context` — or the hook fires with no context to act on. `github.prs_refreshed` is deliberately subscribe-only and is the counter-example, not the template. (learned 2026-08-17, binding: `github.pr_opened` was added this way; a mutation run confirmed that dropping the `PLUGIN_HOOK_EVENT_KINDS` entry is caught only by a test asserting membership directly, and that deleting the `emit_event` call is caught only by a test asserting the event is emitted.)
- For a `placement = "tab"` plugin pane, `open_plugin_tab` (`src/app/api/plugins/panes.rs`) applies the manifest `[[panes]] title` as the new tab's custom name via `set_custom_name` + `crate::logging::tab_renamed`, mirroring `handle_tab_create` in `src/app/api/tabs.rs`. A plugin must not follow `plugin pane open` with a `bora tab rename` call for tab placement — that is dead code now, though the third-party `persiyanov.reviewr` plugin still does it. (learned 2026-08-20, binding.)
- `agent start` does not `exec` the agent binary — it types the executable name as literal shell text into the target pane's already-running interactive shell (`src/app/agents.rs`, via `interactive_shell_command`). That shell is the user's real login shell with rc files sourced, so a local shell function or alias of the same name INTERCEPTS the launch. Consequence: agent launch behavior depends on the operator's rc files, and a wrapper can silently change what runs. This is why `omp` on a machine whose zshrc defines an `omp()` sandbox wrapper launches sandboxed, and why a sandboxed agent in a git worktree cannot read the main checkout's `.beads` (the worktree holds only a `redirect` file pointing there) and falls back to an empty PlanQueue. It is also why `[agents.commands]` can point a kind at a shell ALIAS (`omp = "omp-raw"`): an alias resolves precisely because the name is typed into an interactive shell rather than exec'd, and the same override would be impossible under `exec`. When debugging "the agent launched but behaves wrong", check the operator's rc files before the Rust. (learned 2026-08-21, binding: a full investigation concluded "no-op, bora already execs the raw binary" and was wrong, because the spawn path is a shell write, not a process exec.)
- A unit test that calls `mcp::tools::dispatch` (or anything else that opens the API socket) runs against **whatever bora server is live on the developer's machine**, not against nothing. Several existing tests get away with it because they assert `DispatchError::Tool(_)` and the verb they use fails fast, but the pattern is unsound in two ways and both were shipped and caught in one session: a long-poll verb (`events.wait`, `channel.wait`, `agent.wait`, `pane.wait_for_output`) with no `timeout_ms` blocks forever, turning a 35-second `just check` into an indefinite stall that looks like a hung build rather than a red test; and a side-effecting verb (`agent.start` above all) actually performs its effect in the operator's own session. Test the pure decision instead — extract the branch under test into a function that takes its inputs explicitly and assert on that. When a dispatch-level test is genuinely the point (proving `dispatch` wires a check at all), it MUST use a verb and params that cannot reach past the check, and it SHOULD pass `timeout_ms` anyway so that removing the check produces a fast failure instead of a stall. (learned 2026-08-22, binding.)
- The sidebar's three-pass lockstep contract is `entry_row_height`'s doc comment in `src/ui/sidebar.rs` (find it by symbol; earlier notes cited a line range that has since drifted onto an unrelated function). `workspace_list_visible_count`, `compute_workspace_list_areas`, and `render_workspace_list` MUST all derive every row height by calling `entry_row_height` — never a local constant, never inline arithmetic. `workspace_list_lockstep_passes_agree_for_every_entry_variant` is the characterization test for it and uses non-wildcard `match`es on `WorkspaceListEntry`, so a new variant fails to compile until it is handled there. Note that today every variant is height 1, which means the test cannot be falsified by changing a height — the only mutation that reddens it is making ONE pass disagree with the others. Its value is prospective: it is the net under any change that introduces a row taller than one line. (learned 2026-08-22, binding.)
- A test-only guard that mutates a process-global env var while holding `test_config_env_lock` MUST be exactly one type. It was briefly split per variable (`IsolatedStateDir`, `IsolatedConfigDir`), and the first test that needed both — `project.create`, whose file lives under `config_dir()` and whose channel binding writes a roster under `state_dir()` — constructed both and deadlocked forever, because that lock is a plain non-reentrant `parking_lot::Mutex`. Nothing in the type system prevents nesting, and the symptom is not a red test but a hung suite: a 22-second `just check` became a 30-minute timeout that reads like a broken build. `crate::config::IsolatedDirs` now isolates both variables in one guard, so there is no second guard to nest; isolating a variable a test does not care about costs nothing. Do not re-split it, and apply the same rule to any future env-var guard. (learned 2026-08-22, binding.)
- **A plugin's trust boundary is install time, not call time**, and that is an accepted risk rather than an oversight. `src/api/server.rs` dispatches on `request.method` with no per-caller capability check, so a plugin subprocess has exactly the reach of the `bora` CLI — `workspace.*`, `worktree.*`, `pane.send_input`, `agent.start/prompt`, `channel.*`, `server.reload_config`, all of it. `HERDR_SOCKET_PATH` is set unconditionally in the shared `start_plugin_command` (`src/app/api/plugins/runtime.rs`), which every entry point funnels through: startup hooks, event hooks, actions and panes alike. Since `run_plugin_startup_hooks()` runs at every server start (`src/server/headless.rs`, right after `print_ready_message`) with **zero human interaction**, installing an enabled plugin already grants arbitrary unattended full-wire-API execution. The consequence for design: do NOT add a capability gate to one newly-exposed invocation path while that one stays open. A right-click menu item firing a plugin action requires an explicit human click and is therefore *strictly more* gated than `[[startup]]`; gating only the menu would add friction exactly where the human is present and none where they are absent, which is theater rather than security. Narrowing the boundary itself is tracked separately (a per-plugin scoped RPC table, the one idea worth taking from `deepseek-ai/deepseek-harness`); when that lands, this rule stops being the whole story and must be updated in the same change. Prior-art comparison in `.local/prd/plugin-extensibility.md`. (learned 2026-08-24, binding: this was resolved as the blocking prerequisite of the plugin-menu work, by measuring what already runs unattended rather than by assuming the new path was the riskiest one.)
- **A doc comment promising that consumers "can never drift apart" is not a mechanism; owning the whole classification is.** `check_status.rs` owned only the *failing* conclusion set (`is_failing_conclusion`) and let each caller infer everything else from it. Its comment said the sharing existed so the consumers "can never drift apart" — and they had drifted by the time a fourth consumer appeared, because inferring two states from one predicate is not a shared rule, it is three separate guesses. Concretely: `checks_rollup` derived Passing by elimination, so a `COMPLETED` check with `conclusion: null` (which GitHub really emits) and a `COMPLETED` check carrying any conclusion string added to the API later BOTH displayed as a green tick, and `checks_counts` counted them as passing. Neither case had a test. The fix is `run_state(status, conclusion) -> ChecksRollup` as the single owner, with `check_run_state`/`reduce_run_states` exposed so `open_prs`'s `statusCheckRollup` reduction goes through it too, plus a test asserting the counts/rollup invariant across the whole status x conclusion cross product rather than promising it in prose. When you find yourself writing "so these can never disagree", check whether the code makes disagreement impossible or merely unlikely. Also: unrecognised input must never classify as the optimistic value — green is a claim about someone else's CI, pending is an admission of ignorance, and only one is safe to be wrong about. (learned 2026-08-24, binding: found by a subagent sanity-checking its own new code against the existing path, which is the only reason it surfaced at all.)
- **The headless server runs its own scheduled tick, and it drifts from the App's tick invisibly.** `HeadlessServer::handle_scheduled_tasks_headless` is a deliberately trimmed mirror of `App::handle_scheduled_tasks`, and because nothing forces the two to share code, work added to one does not reach the other. Any new tick work a client-visible surface depends on — polls, refreshes, deadline handlers — MUST go through a shared helper on `App` (see `App::poll_projects_store`) called by BOTH tick paths, never inline in either. A doc comment on one tick naming the other is not a mechanism, for the same reason the check-status rule above is not. (learned 2026-08-25, binding: the `projects.yml` store poll shipped only in the App tick, so in server mode — the only mode anyone runs — projects written while the server was up never grouped in the Project view; the verbs wrote the file correctly and the sidebar simply never reloaded it until a restart. Found by a user report one day after the menu wiring shipped.)
- **`grep` on this machine is a uutils reimplementation, and it parses `()` as an empty regex group.** So `grep -c "unwrap()"` matches every `unwrap` substring — `unwrap_or_default()`, `unwrap_or_else()` — and over-reports. Measured on `src/ui/sidebar/project_view.rs` in one session: 65 unescaped vs 52 with `grep -c 'unwrap\(\)'` or `grep -cF`. Always `-F`, or escape the parens, when counting anything with a call suffix, and the same applies to `todo!()`/`unimplemented!()`. Two neighbouring traps in the same family, both of which produced confident wrong numbers in the same session: a substring check for `TODO` matches the sidebar band name `TODOS`, which reported 19 phantom TODO comments against a real count of 0; and checking whether an `#[allow]` is justified by inspecting the PRECEDING line misses every one justified by a trailing comment on the same line, which reported 8 unjustified against a real count of 0. This sits alongside the two clippy traps already noted above (no re-emit from a cached build; `--message-format short` drops the lint name) — the general rule is that a proxy measurement must be falsified against a known-good and a known-bad input before its output is quoted anywhere. (learned 2026-08-24, binding.)
- **Sidebar bands are a registry, not an enum.** `ProjectSection` is gone. A band is a `SectionDescriptor` in the `const REGISTRY` slice (`src/ui/sidebar/project_view.rs`) carrying its wire name, glyph, label, counter format, bullet style, the `SectionLevel` it may appear at, and a `fn(&mut Vec<WorkspaceListEntry>, &SectionPushCtx)`. Adding a band is a registry entry plus a push function — two sites, down from nine — and `a_sixth_descriptor_renders_through_the_generic_rows_without_touching_the_registry` is the test that keeps it two by declaring a throwaway descriptor that is never registered. Three constraints hold this shape and none is negotiable. `REGISTRY` is `&[&SectionDescriptor]` with no length in its type so `SECTION_COUNT = REGISTRY.len()` and every `[&SectionDescriptor; SECTION_COUNT]` downstream recompute on append; ordering returns that fixed-size array and level filtering returns a borrowing iterator, because this runs per-render × per-pane × per-client and a `Vec` of providers or a `Box<dyn>` would allocate there. `SectionPushCtx` is a `Copy` all-borrowed enum: it exists because the seven push functions had non-uniform signatures (`&[String]` selection, `&Workspace`, a bare slug, an added `&HashSet<String>`) and a single fn-pointer type does not fit them otherwise — normalizing the context is what made the table possible, so do not "simplify" it back into per-band arguments. And `level` is DECLARED by the descriptor rather than imposed by a narrowing function, which is why the old `unreachable!("project_section_order only returns project-level sections")` is gone: it existed solely because the previous shape could not express the constraint. Runtime or plugin-declared bands are deliberately NOT built (the registry is `const`, in-binary); that is the next phase and it needs the generic-row problem solved first, since a new row SHAPE still cascades through `entry_row_height`, both `apply_hidden_filter` matches, `actions.rs`, and `input/sidebar.rs`. (learned 2026-08-24, binding.)
- **A judgement about rendering needs an instrument, not an assertion.** `ui::sidebar::capture` (`#[cfg(test)]`, a CHILD module of `ui::sidebar` so it reaches the private `render_workspace_list` without widening anything) renders a fixed fixture at a fixed size and serializes text AND style as run-length spans. It exists because every other UI test in this repo asserts against its own expectations, which can tell you a rendering changed but never whether it got better — the capture is comparable across two commits, which is the only honest way to judge a visual change. Four text-only flatteners predate it (`src/ui.rs`, `src/ui/tabs.rs`, `src/ui/sidebar.rs`, and an inline closure in `project_view.rs`) and all four discard style, which is exactly why none of them can do this job. Two properties are load-bearing and both have tests: the serialization must NAME what changed and not merely differ (span boundaries move whenever styles differ, so a capture with the modifier omitted still produced a small diff — an illegible one, and the leaf's own mutation set missed it because it mutated the span-grouping comparison instead of the output format), and the capture must not depend on anything that varies between two checkouts. That second one has a trap worth naming: `Workspace::test_new` takes its `id` from a process-lifetime `AtomicU64` and that id reaches rendered text through the `@w<id>p<n>` badge, so any test that snapshots a rendering MUST pin ids explicitly — see the doc comment on `test_new`. A same-process double-capture cannot catch either that or `HashMap` iteration order; run the capture in two separate processes and diff. (learned 2026-08-24, binding.)
- **A whole-frame hash golden is updated by ATTRIBUTION, never by bumping.** `desktop_full_app_semantic_frame_is_characterized` and its mobile sibling (`src/ui/tab_surface.rs`) assert a SHA-256 of the encoded `FrameData`, so any intended rendering change breaks them and the hash itself tells you nothing about what changed — which means pasting the new digest is indistinguishable from pasting over a real regression. The technique that makes the update honest, and which is not obvious: temporarily replace the `assert_eq!` with a probe that prints the sidebar columns out of `frame.cells` (row-major, so `cells[row * frame_width + col]`, NOT a `rows` field — there isn't one), then run the SAME probe against the pre-change build by swapping only the one file that changed the rendering — `git show HEAD:src/ui/sidebar.rs > src/ui/sidebar.rs`, probe, restore from a `cp` backup. One recompile, no worktree, and it produces a real before/after diff you can put in the comment. Doing this caught a doubled `◰` glyph that looked exactly like a regression the round had introduced and was in fact pre-existing (workspace-type glyph followed by agent glyph) — a conclusion that was not reachable by reading either the diff or the new rendering alone. The comment above the assertion MUST carry the before/after rows and state which assertions still hold; treat a golden update with no such comment as unreviewed. (learned 2026-08-24, binding.)
- **When an HTML mock is the approved bar, the bar is its CSS, not its screenshot.** Two real fidelity defects in the Project-view v3 work survived a blind critic round *and* a full green gate, and both were found in one minute by reading `sidebar-mocks.html`'s stylesheet instead of looking at the rendering: `.sec .st { float: right }` said the state cluster pins to the row's right edge while the implementation left it inline after the branch, and `.behind { color: var(--yellow) }` said red belongs to `.fail` alone while the implementation spent red on "behind origin", which is exactly the colour budget a real CI failure needs. Neither is visible in a screenshot at sidebar width, because a short workspace name puts the inline cluster roughly where a floated one would land and one amber-vs-red arrow reads as "a warning colour" either way. The blind critic could not catch them either: it judged a lossy text extract, so it flagged the *absence* of colour rather than the wrong colour. Grep the mock for `float`, `text-align`, `var(--` and the declared palette, and diff that list against the code's style table — the declarations ARE the spec, in a form that is checkable, and a picture of them is not. (learned 2026-08-26, binding.)
- **Identity that is DERIVED cannot answer a question with two right answers.** A workspace's project was derived entirely from its path — `repo_identity` + `checkout_key` + subdir prefix in `workspace_matches_member` — and `project_view_entries` handed each workspace to the first project that matched, walking a `BTreeMap`, so slug-alphabetical order silently decided ownership. That is correct exactly while a directory belongs to one project, and the owner's real `projects.yml` has `worktrees: all` on every member, so one entry claims every worktree of a repo. Declare a second project on the same repo and the alphabetically-first one takes all of it; there was no way to say otherwise, because nothing on `Workspace` could hold the answer. The fix shape generalises and is worth reaching for directly next time: keep the derivation as the fallback, add an explicit binding that wins over it, and make a stale binding fall THROUGH to the derivation rather than orphan the row — so the feature degrades to the old behaviour instead of to an empty group. Two traps found while fixing it. Persist the binding on BOTH sides or it evaporates on restart, which is the worst shape of the bug because it works until the operator restarts: `restore_workspace` (`src/persist/restore.rs`) is the only `Workspace` struct literal outside `src/workspace.rs`, and a subagent found it after the lead's own `grep "Workspace {"` returned 239 noisy hits — enum variants, `impl` blocks — and the lead read that noise as "nothing real is there". And when a derivation must break a tie at all, rank by specificity (`WorktreesScope::This` over `All`, deeper subdir over shallower) and let sort order decide only what specificity leaves tied; alphabetical order as the *primary* rule is how this bug read to the user as random. Note that `visual_group` is NOT this concept and must not be merged into it: it is free text the user types, consumed only by Flat/Repo grouping, whereas `project` is a validated key into `projects.yml` consumed only by Project view. (learned 2026-08-26, binding.)
- **A new row variant must emit every hit area its old shape emitted, and one missing area reads to the user as several unrelated bugs.** Project view's `SectionRow` replaced the `WorktreeRow` + indented `Workspace` pair and pushed only a `ProjectRowHitArea` in `workspace_list_areas_for_entries` (`src/ui/sidebar.rs`) — never a `WorkspaceCardArea`. Everything workspace-scoped keys off that second vec, so three symptoms landed at once and none of them looked like a geometry problem: right-click fell past `workspace_at_row` into the narrow `ProjectRowTarget::Section` arm and offered only the project-membership items ("right-click shows one option, everything that used to be there is gone"); no press was recorded, because the Project-view branch returns early via `handle_project_row_click` BEFORE `workspace_presses.insert`, so `DragTarget::WorkspaceReorder` could never open ("I can't drag the worktrees any more"); and the render arm painted no selection or active fill, so a click produced no visible feedback (reported as "hover doesn't work" — note bora has no mouse-hover state anywhere, so any such report means the selection/active fill, and looking for hover code wastes the trip). Resist diagnosing them separately: when several affordances die together on one row type, check what the row emits before reading any of the handlers. Two traps in the fix itself, both of which cost a round. A guard written for the old shape survives and re-kills the feature one layer down — `workspace_move_block_params` (`src/app/input/sidebar.rs`) refuses linked worktrees unconditionally, which is right in Flat/Repo where a worktree is an indented child but wrong in Project view where it is its own top-level row, and the `roots` filter directly below it *already* treated `SectionRow` as a root, so the function contradicted its own stated intent and swallowed the drop after the drag opened. And the fix widens a context-menu kind, which is a characterization change, not a regression: keep the item-level assertions untouched and let them prove nothing was traded away — the menu kind widened from `ProjectMemberTargets` to `GitWorkspace` while every membership item stayed. (learned 2026-08-26, binding.)
- **`git stash push -- <subset of paths>` on a tree whose feature is UNCOMMITTED does not give you a baseline; it gives you a tree that does not compile.** Attempting the whole-frame-golden probe above, the "swap only the file that changed" step was run as a partial stash — and `HEAD:src/ui/sidebar.rs` turned out to contain zero occurrences of `SectionRow`, because the entire Project view was uncommitted working-tree work along with ~10 other modified files. The stash tore one half out of a coherent change set and 25 compile errors surfaced in `src/ui/mobile.rs`. ALWAYS run `git log --oneline -3` and `git show HEAD:<file> | grep -c <the symbol you are reverting past>` before assuming HEAD is a usable baseline; when the feature is uncommitted there IS no earlier build to probe, and the honest attribution is the current probe plus the before/after rows the previous golden's own comment already recorded. Recovery is `git stash pop` immediately — verify with a grep for one symbol from each touched file, not by eye. (learned 2026-08-26, binding.)
- **A colour reserved for a failure state is reserved across the whole surface, not per row.** `p.red` was spent on the Project header's counter to satisfy an ask for "half purple, half pink", reasoned as safe because Catppuccin's `red` is a soft rose (`#f38ba8`) and that row carries no state cluster to collide with. Wrong scope: the harm the reservation names is the reader's eye scanning the sidebar for red, so a rose tone on every project header trains that eye to ignore the hue, and a real CI failure gets harder to spot — exactly the outcome the rule exists to prevent, reached without any single row containing a contradiction. The ask needed no second colour at all: `p.mauve` is `Rgb(203, 166, 247)`, purple leaning pink, which IS "half purple, half pink" in one swatch. Related, and the actual reason such a colour can look absent: `Palette::terminal()` (the 16-colour theme, `src/app/state.rs`) had `mauve: Color::Gray` identical to `overlay0`, and `surface0: Color::Reset` identical to `sidebar_bg` — so on that theme every mauve accent read as muted text and every "slightly lighter" row fill was no fill. Before concluding a colour is not landing, check the palette the operator's theme actually resolves to; before adding a palette field, check whether an existing one already IS the requested hue. (learned 2026-08-26, binding.)
- **A derived column must be anchored to data both passes share, never to the geometry each pass is handed.** `pane_dots_layout` (`src/ui/sidebar.rs`) returns the column of every pane dot on a `PaneDotsRow`, and both `render_workspace_list` and `workspace_list_areas_for_entries` consume it — a third lockstep consumer alongside `entry_row_height`'s three passes. The first version right-pinned the dots to the row's right edge, i.e. computed `width - dots_width`, which made every column a function of the row GEOMETRY. The two passes reached the function with different `body` rects, and the hit areas landed on blank space past the glyphs: a click that focuses a neighbouring pane, or nothing, with no error anywhere. Sharing one helper was not enough, because the helper's *input* differed. Re-anchoring the columns to the entry's own `name` fixed it structurally — `name` comes off the `WorkspaceListEntry` that both passes are walking, so there is nothing left to disagree about, and the row pads on the right to keep its exact-width invariant. Generalise: when two passes must agree on a position, derive it from the DATA they both hold, not from the layout rect each is separately given; a shared function over unshared inputs is not a shared answer. Note also that this was caught only by the agent's own new test (`pane_dots_row_hit_areas_land_on_the_rendered_dots_own_columns`) asserting hit rects against the RENDERED buffer rather than against re-derived arithmetic — a test that recomputed the expected columns would have passed. (learned 2026-08-26, binding.)
- **Glyph coverage is a property of the font FILE, per codepoint — the Unicode block proves nothing, and this nearly shipped a regression.** A pane-state spinner rendering as tofu was diagnosed as "Braille (`U+28xx`) is missing from the operator's font", and the proposed fix was `◐◓◑◒` (`U+25D0..25D3`), justified because the idle `○` (`U+25CB`) visibly rendered and is in the same Geometric Shapes block. Measured against the actual file (`~/Library/Fonts/JetBrainsMonoNerdFontMono-Regular.ttf`, parsing the `cmap` table directly — `fontTools` is not installed, ~50 lines of `struct` unpacking): Braille `SPINNERS` **10/10 present**, `SAND` **35/35 present**, and the proposed `◐◓◑◒` **0/4**. The "fix" would have replaced working glyphs with real tofu, and the block-adjacency argument was worthless — `✱` (`U+2731`) is also absent while `○`, `●` and `↑` are present, in overlapping ranges. Before changing a glyph for coverage reasons, read the operator's `font-family` out of their terminal config, find the file, and enumerate the codepoints; and remember there are TWO spinner sets here (`SPINNERS` and `SAND`, both in `src/ui.rs`) plus the `glyph_style` sets in `src/config/sidebar.rs`, so "the spinner" is four things. (learned 2026-08-26, binding.)
- **On this machine `~/.config/bora/config.toml` is a read-only symlink into the nix store; the source is `~/Sites/dotfiles-2026/dotfiles/bora/config.toml`.** Telling the operator to add a key to the former cost him a full home-manager switch that rebuilt the identical config and changed nothing, and the key path itself was correct — only the file was wrong. Verify with `ls -l` before instructing anyone to edit a dotfile here; the same holds for `~/.config/ghostty/config`. Two useful follow-ons measured at the same time: `apply_live_config` (`src/app/mod.rs`) does update `sidebar_project`, so `bora server reload-config` applies `glyph_style` with no restart and no rebuild — a `bora server stop` is more than the situation needs; and there is NO nix package for the bora BINARY, only its config and helper scripts, so `~/.local/bin/bora -> target/release/bora` from `just install` is the only binary on `PATH` and a "stale version in dotfiles" is never the explanation. (learned 2026-08-26, binding.)
- **A test fixture that cannot express its scenario reports a production bug that does not exist.** `repo_shown_true_again_in_a_different_project_group` declared two projects, `alpha` on checkout A and `beta` on checkout B, and asserted the same repo shows its name in each group. It failed — and the production code was right. Both checkouts shared an origin, so they shared a `repo_identity`, and a member with `worktrees: all` claims EVERY worktree of that repo, so slug-alphabetically-first `alpha` took both and the fixture silently built ONE group of two rows. That is the behaviour already recorded under "Identity that is DERIVED cannot answer a question with two right answers" a few entries above, which is exactly why the explicit `Workspace::set_project` binding exists: the scenario is only expressible through it, and using it makes the test exercise the real production path rather than a hypothetical one. When a brand-new test fails, dump the fixture's actual output (the assertion message here carried the whole entry list, showing one `ProjectRow { name: "alpha", live: 2 }`) before touching production code — a fixture that cannot reach the state under test measures nothing, and "fixing" the code to satisfy it would have broken the real rule. (learned 2026-08-26, binding.)

### Removed — do not reintroduce
<!-- Tombstones: things deleted on purpose. Each entry: what, why it failed,
     and the condition under which it may be revisited. -->

## Release Channels

This section is maintainer-only for release actions. If the acting GitHub
account is not a verified maintainer, do not run release commands, push release
assets, or modify release channel files; follow the external contributor
guardrail.

Herdr has one main branch and two update channels. Stable and preview both build from `master`; there is no long-lived preview branch.

Normal users default to stable. Stable docs are `/docs/`, stable updates use `website/latest.json`, and Homebrew/Nix stay stable-only.

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

Preview releases are GitHub prereleases produced by `.github/workflows/preview.yml` on manual dispatch and the Wednesday/Friday schedule. The workflow updates `website/preview.json`, which the website build publishes as `/preview.json`. Do not hand-edit `website/preview.json`; fix the workflow or `scripts/preview.py` and rerun Preview.

Stable releases use:

```bash
just check
just release 0.x.y
```

Before stable release, run `/pre-release-audit`, finalize `docs/next`, and run `just pre-release-check` to validate the staged docs, website build, and render scaling. `just release` prepares the changelog and release commit, tags it, and pushes the tag. GitHub Actions builds binaries, creates the GitHub release, closes released issues, snapshots and promotes the tagged docs, and updates `website/latest.json`.

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

## Beads durability in this repo — read before trusting the backlog

This repo's `bd` runs in `dolt_mode: server` against the shared central server
`dolt.bugtoprompt.com:3307` as user `bd_ops`. Two facts that a healthy-looking
`bd ready` does not tell you, both measured 2026-08-24:

- **There is no off-machine database backup, and the two supported routes are
  both blocked from here.** `bd dolt push` returns
  `Error 1105 (HY000): command denied to user 'bd_ops'@'%'` — `refs/dolt/*` does
  not exist on `origin` and cannot be seeded from a client, because the push
  would run server-side and that user has no push grant (nor would the VPS hold
  this machine's GitHub credentials). `bd backup init <local dir>` does not
  substitute: the server writes to *its* filesystem, which is exactly what bd's
  own `auto-backup skipped — server filesystem differs from client` line on
  every command is reporting. The unblocking action needs a credential only the
  owner can supply: a DoltHub remote (`bd backup init
  https://doltremoteapi.dolthub.com/<user>/<repo>` plus `DOLT_REMOTE_USER` /
  `DOLT_REMOTE_PASSWORD`), or a push grant for `bd_ops` on the server.
- **The only off-machine copy is the tracked `.beads/issues.jsonl`, and it is
  issue-level, not a database.** It carries labels, dependencies and comments
  per issue; it carries no Dolt branches, no commit history, no working-set
  state. It is also *only as fresh as the last `bd export`* — it sat 2 days
  stale at one point, showing all 36 issues as `open` while the live DB had
  closed 5 epics, so an agent reading the file instead of the DB concludes the
  opposite of the truth. Re-export before trusting it, and never read it as
  current state. `.beads/interactions.jsonl` (untracked by the deliberate
  decision in `f3032554`) is the local audit trail and is the thing to read when
  the server is unreachable — it records every status change with its reason.

Also: `.beads/dolt/` is the default data-dir. If a database root ever appears
*there* (a `.dolt`/`.doltcfg` pair directly under it rather than under
`.beads/dolt/<db>/`), it is inert while the mode is `server` but becomes a
silent time machine the moment anyone sets `dolt.mode: embedded` — bd serves
that empty database and reports healthy. One was found and moved out on
2026-08-24; its `config.yaml` also carried `listener.port: 3307`, the central
server's port on loopback, which is level 3 of bd's port-resolution chain.
(learned 2026-08-24, binding.)

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:970c3bf2 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   bd dolt push
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep persistent project memory in Beads via `bd remember`; do not create ad hoc memory files.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/SYNC_CONCEPTS.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->
