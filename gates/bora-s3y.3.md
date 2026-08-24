# Gates: bora-s3y.3 — TODOS and NOTES project-level sections

Scope: TODOS (n/m = done/total) and NOTES sections render between the project
row and its worktrees, using the E3 section shapes, height-1 rows,
collapsed_space_keys namespaces. Owned paths: `src/ui/sidebar.rs` (and
`src/ui/sidebar/` submodules), this file. NOT owned: stores/verbs (landed),
worktree-level sections.

- [x] G1: TODOS section renders at project level with n/m = done/total,
  sourced from the todos store; blocked todos excluded from an actionable
  view/filter. Tests pin counts on a mixed fixture.
  CHECK: cargo nextest run -E 'test(/todos_section|todos_notes|todos_summary|section_todos|todo.*section/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.014s] 5 tests run: 5 passed, 3995 skipped

- [x] G2: NOTES section renders at project level listing scratchpad docs;
  sections appear between the project row and its worktrees (entry ordering
  pinned by test).
  CHECK: cargo nextest run -E 'test(/notes_section|todos_notes|list_docs|section_header_notes|section_notes|scratchpad.*section/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.019s] 9 tests run: 9 passed, 3991 skipped

- [x] G3: lockstep contract intact (entry_row_height across all three passes),
  rows height-1, collapsed_space_keys namespaced; render pure (reads via
  cached/snapshot state, no store I/O per frame).
  CHECK: cargo nextest run -E 'test(/lockstep|entry_row_height|collapsed/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.046s] 31 tests run: 31 passed, 3969 skipped

- [x] G4: full suite green after the change (lead-run).
  CHECK: cargo nextest run --no-fail-fast 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  23.823s] 3999 tests run: 3999 passed, 1 skipped
