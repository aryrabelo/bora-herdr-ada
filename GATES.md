# Gates: folders selected-row background

Scope: In sidebar Folders view only, the workspace row that carries the active
`▎` marker (and any navigate-selected row) renders with a lighter background
fill than the other rows. No other view mode changes.

- [x] G1: Folders mode — the active row (marker row) fills with `active_row_bg`; other workspace rows and the group header stay unfilled; the `▎` bar still draws on the filled row.
  CHECK: cargo nextest run -E 'test(folders_active_row_gets_background_fill_and_others_do_not)' 2>&1 | tail -5
  EXPECT: /1 test run: 1 passed/
  EVIDENCE: ──────────── | Summary [   0.019s] 1 test run: 1 passed, 4226 skipped

- [x] G2: Scope guard — Project view's 2-line `PaneDotsRow` blocks keep the GC3 marker-only statement (active row background stays Reset).
  CHECK: cargo nextest run -E 'test(project_view_active_block_keeps_marker_only_fill)' 2>&1 | tail -5
  EXPECT: /1 test run: 1 passed/
  EVIDENCE: ──────────── | Summary [   0.011s] 1 test run: 1 passed, 4226 skipped

- [x] G3: Scope guard — Flat view unchanged: existing GC3 tests (active-but-not-selected has no bg) still pass untouched.
  CHECK: cargo nextest run -E 'test(navigate_selection_keeps_its_existing_background) or test(selected_active_workspace_resolves) or test(pane_dots_row_block_paints_selection)' 2>&1 | tail -5
  EXPECT: /4 tests run: 4 passed/
  EVIDENCE: ──────────── | Summary [   0.012s] 4 tests run: 4 passed, 4223 skipped

- [x] G4: Mutation check — gating the new fill off (or widening it) makes G1/G2 redden, proving the tests defend the scope.
  CHECK: manual mutation run, revert after
  EXPECT: /tests fail under mutation, pass after revert/
  EVIDENCE: mutation 1 (gate widened to every mode): project_view_active_block_keeps_marker_only_fill FAILed; mutation 2 (gate disabled): folders_active_row test FAILed (0 passed, 1 failed). Both reverted; 6-test battery green after revert.

- [x] G5: Repo gate — `just check` passes.
  CHECK: just check 2>&1 | tail -5
  EXPECT: /OK/
  EVIDENCE: OK | docs reminder: if this changes user-facing behavior, make sure the relevant release docs are updated or called out before release.
