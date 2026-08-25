# Gates: integration — wave 1 (B1 + A1 + A2)

Scope: the three wave-1 leaves merge into one coherent tree that builds, passes the full suite, and
ships with the click path wired, the accepted risk written down, and the release contract honoured.

- [x] GI1: the full suite is green on the merged tree. Not a targeted filter — the whole thing,
      including the maintenance script tests.
  CHECK: just check 2>&1 | grep -E "Summary|tests run|FAILED|^error" | tail -4
  EXPECT: /0 failed|passed/
  EVIDENCE: `Summary [ 27.625s] 4052 tests run: 4052 passed, 1 skipped` / `OK` / `OK (skipped=1)`
      — re-measured AFTER the 0.43.0 version bump. The earlier reading recorded here was a stale
      failure: bumping the version changed `Cargo.lock`, and `--locked` refuses to update it, so the
      lint recipe failed for a reason unrelated to the code. Refreshed the lock, then re-ran.

- [x] GI2: the capability decision is recorded in AGENTS.md as a dated binding rule, naming
      install-time trust as the real boundary and `[[startup]]` as the reason the menu path needs no
      gate. Accepted risk that is not written down is indistinguishable from an oversight, and the
      next agent to touch this will otherwise re-litigate it.
  CHECK: grep -c "trust boundary is install time" AGENTS.md && grep -c "learned 2026-08-24, binding" AGENTS.md
  EXPECT: /[1-9]/
  EVIDENCE: AGENTS.md:402 — "**A plugin's trust boundary is install time, not call time**, and that
      is an accepted risk rather than an oversight", citing server.rs dispatch, runtime.rs setting
      SOCKET_PATH_ENV_VAR unconditionally, and headless.rs running startup hooks with zero human
      interaction; closes "do NOT add a capability gate to one newly-exposed invocation path while
      that one stays open". Dated marker present (4 in file). NOTE: this gate's original CHECK
      searched the literal "install-time trust", which the written rule does not contain — it says
      "trust boundary is install time". Measured 0, instrument corrected to the load-bearing phrase
      plus the dated marker. The gate's intent was met; its proxy string was wrong.

- [x] GI3: a sidebar PR row is clickable end to end. The lead wires the hit-test in
      `src/app/input/mouse.rs`, so clicking a sidebar PR row opens it in a worktree exactly as the
      right panel's right-click already does. A row that renders but does nothing is a worse outcome
      than no row.
  CHECK: cargo nextest run --locked open_pr_row_click 2>&1 | grep -c "1 passed"
  EXPECT: /[1-9]/
  EVIDENCE: Wired as `ProjectRowTarget::OpenPr { ws_idx, number }` (`src/app/state.rs`), emitted from
      the geometry walk (`src/ui/sidebar.rs`, the `PrRow` arm) and handled in
      `handle_project_row_click` (`src/app/input/mouse.rs`) by setting the SAME
      `request_open_pr_worktree` field the right panel's "Open in worktree" sets — so the two reach
      one destination by construction rather than by agreement. `ws_idx` resolves once per project
      member at band-build time via `ws_idx_for_identity`, never per row and never in the geometry
      walk, because it is a scan over `app.workspaces` and that walk is per-render x per-pane x
      per-client. Note the plan said to reuse `ContextMenuKind::RepoPr` for this; that is the
      RIGHT-CLICK menu, and a left-click row needs a `ProjectRowTarget`, so the reuse landed one
      level lower — at the shared request field rather than the menu kind. Tests: `open_pr_row_click_requests_the_pr_worktree`,
      `project_view_geometry_pr_row_with_ws_idx_targets_open_pr`,
      `project_view_geometry_pr_row_without_ws_idx_gets_no_hit_area_but_advances_row_y`, and the
      full-AppState `workspace_list_lockstep_pull_requests_agree_across_passes` now asserting
      `OpenPr { ws_idx: 0, number: 7 }`. All green in the 4052-test run.

- [x] GI4: `Cargo.toml` version bumped and `docs/next/CHANGELOG.md` has an entry per user-facing
      change (plugin menu items; PR sidebar band). Binding repo rule: an unbumped build is
      indistinguishable from the previous one at runtime, since the binary self-reports its version.
  CHECK: grep -n "^version" Cargo.toml
  EXPECT: /0\.4[3-9]\./
  EVIDENCE: 3:version = "0.43.0"

- [x] GI5: no leaf left a `TODO`, placeholder, stub, or `#[allow]` without justification in the files
      it touched.
  CHECK: git diff -U0 | grep '^+' | grep -cE "#\[allow|todo!\(\)|unimplemented!\(\)|(//|#)[[:space:]]*TODO\b"
  EXPECT: 0
  EVIDENCE: 0 added across the whole wave diff — 0 `#[allow]`, 0 `TODO` comments, 0 `todo!()`/
      `unimplemented!()`. Whole-file state: 15 pre-existing `#[allow]`, all 15 carrying a
      justification, and 0 real `TODO` comments. INSTRUMENT CORRECTED TWICE, both errors mine: the
      original `grep "TODO\|todo!()\|unimplemented!()"` reported 18-19, every single hit being the
      band name `TODOS` (`grep -o TODO` = 19, `grep -o TODOS` = 19, real TODO comments = 0); and a
      follow-up check for unjustified `#[allow]` reported 8 because it only inspected the PRECEDING
      line, while all 8 carry a trailing comment on the same line. The gate now measures added lines
      in the diff, which is what "no leaf LEFT" actually means.

- [x] GI6: production `unwrap()` count is still zero on `--bins`. The repo denies
      `clippy::unwrap_used` there, and the two known measurement traps apply: clippy does not
      re-emit from a cached build (`touch src/main.rs` first), and `--message-format short` omits the
      lint name.
  CHECK: touch src/main.rs && cargo clippy --bins --locked -- -D clippy::unwrap_used 2>&1 | grep -c "used \`unwrap()\`"
  EXPECT: 0
  EVIDENCE: 0

- [x] GI7: every leaf's own gates file is fully checked with real evidence, or carries an explicit
      `ABANDON:` line. A leaf reporting `N of N` while its file still says `pending` is an unmet gate
      regardless of what it reported.
  CHECK: grep -c "EVIDENCE: pending" gates/bora-1e9.1.md gates/bora-yw6.1.md gates/bora-yw6.2.md
  EXPECT: /bora-yw6.2.md:0/
  EVIDENCE: all three wave-1 leaf files: 0 `EVIDENCE: pending` lines, 0 unchecked boxes, 0 ABANDON.
      Counted per file — bora-1e9.1.md 7 checked / 0 unchecked / 0 pending, bora-yw6.1.md 8/0/0,
      bora-yw6.2.md 10/0/0 — for 25 leaf gates met. INSTRUMENT CORRECTED: the original CHECK invoked
      `gate-check.mjs`, which (a) globs every gates file in the directory regardless of the paths
      passed to it, so it dragged in five earlier rounds' files and reported their historical unmet
      gates as if they were this wave's, and (b) recursed, because gate-check running THIS gate ran
      gate-check again. Both made the result unreadable rather than wrong-but-clear.

- [x] GI8: the leaves' independent mutation claims are re-verified by the lead, not trusted. Pick one
      behavioural test per leaf, revert the line it defends, confirm it fails, restore with `cmp`.
      Self-reported mutation proofs are exactly the claim class that has been wrong before in this
      repo.
  EVIDENCE: Four mutations run by the lead, each backed up with `cp`, restored, and confirmed
      byte-identical with `cmp`:
      (1) B1 — dropped `plugin.enabled &&` at `plugins/mod.rs:734`. Reddened exactly the two tests
      B1 predicted: `plugin_action_context_disabled_plugin_contributes_nothing` (state.rs) and
      `plugin_action_context_dagr_via_general_mechanism_still_offers_entry` (mouse.rs). 8/8 green
      after restore. NOTE: my FIRST attempt at this mutation reddened nothing and looked like B1
      had over-claimed. The pattern I matched carried 12 spaces of indentation and matched a
      DIFFERENT function elsewhere in the file (there are two identical `.filter(...)` lines, at
      :585 and :734); `cmp` said the file had changed while `sed -n 734p` still showed the original,
      which is what exposed it. A mutation that reddens nothing is a claim about the instrument
      before it is a claim about the test.
      (2) A1 — `check_run_state(status, conclusion)` -> `check_run_state(status, Some("SUCCESS"))` at
      `open_prs.rs:53`. Reddened 3 of 14: `each_failing_conclusion_is_failing`,
      `completed_with_null_conclusion_is_pending`, `precedence_failing_beats_pending_and_passing`.
      14/14 green after restore.
      (3) lead's own click path — dropped the `request_open_pr_worktree` assignment in `mouse.rs`.
      Reddened `open_pr_row_click_requests_the_pr_worktree`.
      (4) lead's own geometry guard — emitted the hit area unconditionally with
      `ws_idx.unwrap_or(0)`. Reddened
      `project_view_geometry_pr_row_without_ws_idx_gets_no_hit_area_but_advances_row_y`. This is the
      mutation worth having: defaulting a missing `ws_idx` to 0 would create the worktree in
      whatever repo happens to sit at index 0.

- [x] GI9: DOX pass — the closest owning AGENTS.md updated for anything that changed purpose, scope,
      contracts, or constraints, and stale text removed. Report docs intentionally left unchanged and
      why.
  CHECK: grep -c "learned 2026-08-24, binding" AGENTS.md
  EXPECT: /[3-9]/
  EVIDENCE: Three dated binding rules added to Code Conventions: the plugin trust boundary being
      install time (AGENTS.md:402), "a doc comment promising consumers cannot drift is not a
      mechanism" from the checks-rollup bug (:403), and the uutils `grep` `()`-as-empty-group trap
      plus its two neighbours, `TODO` matching the `TODOS` band name and preceding-line-only
      `#[allow]` justification checks (:404). Stale text REMOVED, not just added to: the
      `ContextMenuKind::RepoPr` doc comment claimed the variant was a sidebar row when the sidebar
      had no PR row at all and its only construction site is the right panel, and both its sentences
      were wrong (committed separately as 797aa037); `docs/next/CHANGELOG.md`'s unreleased "Open
      dagr" entry described the special case B1 deleted and was rewritten to describe the general
      mechanism, since shipping a changelog for a feature we did not build is worse than no entry.
      Intentionally left unchanged: no rule was added for the PR band or the section fifth variant —
      the three-pass lockstep rule at :400 and the multiplicative-performance section already govern
      them, and restating a parent rule in a child doc is what this repo's DOX contract forbids.

- [x] GI10: beads `bora-1e9` and `bora-yw6` closed with reasons, or left open with the remaining work
      named.
  CHECK: bd list --status open --json 2>/dev/null | jq -r '.[].id' | grep -cE "bora-(1e9|yw6)$"
  EXPECT: 0
  EVIDENCE: Both closed with full reasons. `bd list --status open` now returns 6 beads, neither of
      them these: bora-by6 (the promoted attachment registry, p1), bora-b3o (per-plugin scoped RPC
      table, filed from the deepseek-harness finding), bora-wfi, bora-unw, bora-4rk, bora-4rz.

<!--
Wave 2 (C, named-slot registry) is scoped only after B1's mechanism exists, per PLAN.md. It gets its
own gates file; do not fold it into this one.
-->
