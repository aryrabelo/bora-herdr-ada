# Gates — bora-rlu.2 — indented workspace rows show unique tokens, not the repo name

Lead dispatch did not include a pre-created gates file; ledger created by the
builder from the bead's verbatim acceptance criteria. Evidence lines reference
the final working tree (sidebar.rs is 6207 lines). Line numbers in the bead
description predate the rlu.1 truncation work that landed in the same file;
symbols were located by name, not by line.

## Gate 1 — Indented child rows stop using the repo-derived display name
- Workspace arm computes `full_label` via `indented_child_label` when
  `*indented && agent_badge.is_some()`: src/ui/sidebar.rs:2473-2478, with the
  badge lookup hoisted above the label (`agent_badge = workspace_agent_label(
  ws, &app.terminals)` at :2473) so it runs once per row and is reused for the
  row's ` @name` suffix at :2561.
- `indented_child_label` (src/ui/sidebar.rs:537-548): a custom name passes
  through verbatim and suppresses the logic entirely; otherwise the label is
  the branch display label ONLY when it differs from the parent header's
  printed label, else the empty string. The `@wNpN` badge itself is the
  existing `agent_suffix` span the arm already draws beside the label.
- Badge-less guard: a plain-shell pane reports no pane detail (no
  `agent_name`, no detected/hook agent — `Tab::pane_details` `?`-filters it),
  so `workspace_agent_label` returns None; such a child keeps
  `display_name_from` — a duplicate name still says more than an anonymous
  empty label (src/ui/sidebar.rs:2474-2478).
- `grouped_child_display_label` NOT reused (mobile-scoped, returns the branch
  name — the exact collision this bead fixes); it and src/ui/mobile.rs are
  untouched.
- The label flows through the arm's existing `truncate_end(&full_label, avail)`
  budget (src/ui/sidebar.rs:2581; `truncate_end("")` is a no-op per
  src/ui/text.rs:11-14) — rlu.1's truncation work untouched.
- EVIDENCE: src/ui/sidebar.rs:2473-2478, src/ui/sidebar.rs:537-548.

## Gate 2 — Branch repeats only when the parent header did not print it
- Render loop tracks `parent_branch` (the label the most recently visited
  header printed; None = none): declared src/ui/sidebar.rs:1979-1982, updated
  in the GroupHeader (:1991, None), ProjectHeader (:2039, folded
  `ProjectHeaderBranch.label` or None), BranchHeader (:2155, its label), and
  HiddenHeader (:2293, None) arms. Header→child adjacency holds by
  construction in `workspace_list_entries_inner`/`apply_hidden_filter`; a
  header scrolled off the top leaves `parent_branch` None, and its children
  correctly re-show the branch (the header is not on screen to repeat).
- Child compares its `branch_display_label(ws.branch())` against that value —
  the same display form the headers print (`worktree/` stripped on both
  sides, since headers store `branch_display_label(...)` at build time).
- EVIDENCE: src/ui/sidebar.rs:1979-1982, :1991, :2039, :2155, :2293,
  :2474-2476.

## Gate 3 — Tests pin exact row content (red under revert to raw display_name_from)
- `indented_same_branch_children_render_distinct_badge_rows`
  (src/ui/sidebar.rs:3998): three auto-named workspaces on one checkout —
  branch "first" (folds into the project header) and two on "main" (under a
  `├──` branch sub-header). Asserts the exact rows `╭─herdr [first]` /
  `│   ◰  @<id0>p1` / `├── main` / `│   ◰  @<id1>p1` / `╰── ◰  @<id2>p1` and
  `assert_ne!` on the two same-branch siblings. Red under revert: the old
  rows render the same cwd-derived name where these expect badge-only rows;
  pinning both siblings catches a revert of either the folded-header or the
  branch-header parent tracking.
- `indented_child_custom_name_renders_verbatim` (src/ui/sidebar.rs:4052):
  custom-named child renders `╰── ◰ release-hotfix @<id>p1` verbatim beside a
  badge-only auto sibling. Pins the "custom names unchanged" contract.
- `indented_child_under_collapsed_header_shows_branch_header_omitted`
  (src/ui/sidebar.rs:4095): collapsed header prints no branch (`╭─herdr ◰`),
  so its active child shows it: ` ◰ main @<id>p1`. Red if the branch-differs
  logic is removed (the row would drop "main").
- `indented_child_without_agent_badge_keeps_display_name`
  (src/ui/sidebar.rs:4135): no detected agents anywhere → children keep their
  cwd-derived display names (`│   ◰ first` / `│   ◰ second` / `╰── ◰ third`).
  Red if the badge-less guard is removed (rows go empty).
- Fixture helper `detect_agent_on_root_pane` (src/ui/sidebar.rs:3988) gives
  test terminals a detected agent — a bare `TerminalState` yields no pane
  detail, hence no badge.
- EVIDENCE: src/ui/sidebar.rs:3998, :4052, :4095, :4135, helper at :3988.

## Gate 4 — Lockstep contract and row geometry untouched
- Label content only: no change to `entry_row_height`, entry construction,
  `compute_areas`, or row counts — the lockstep contract
  (entry_row_height + visible_count + compute_areas + render) is
  unreferenced by this diff. No `AppState` mutation in render;
  `parent_branch` is render-loop-local. No new pane-scaled work: the badge
  lookup was already called once per row (now moved, not duplicated).
- EVIDENCE: diff touches only the Workspace arm label, the four header arms'
  one-line parent-branch updates, the helper, and tests.

## Gate 5 — Repo tests green
- Not runnable by the builder (dispatch forbids cargo). Static inspection:
  every existing test that renders Workspace rows uses custom-named fixtures
  (`Workspace::test_new` sets `custom_name: Some(name)`, so `git_space_member`
  members are custom-named → verbatim path, unchanged output) or the no-panic
  CJK test; `indented_child_label`'s auto-named path is exercised only by the
  four new tests. The `tests/` integration suites do not render the sidebar
  workspace list.
- EVIDENCE: pending — lead runs `cargo test`.

## Non-goals respected
- rlu.1's `truncate_end` budget arithmetic in the ProjectHeader/BranchHeader
  arms untouched; the child label reuses the Workspace arm's existing budget.
- `grouped_child_display_label` and src/ui/mobile.rs untouched.
- Non-indented (top-level/flat-mode) rows keep `display_name_from` verbatim.
