# Gates: bora-55c.3 — COMMANDS section + delete Programs band

Scope: the worktree-level COMMANDS band goes live — n/m = tagged panes alive /
declared, items are the project's selected commands with running state, click
launches — and the legacy Programs band (row count + band rect + render +
hit-test rect) is deleted, not duplicated. Owned paths:
`src/ui/sidebar.rs`, `src/ui/sidebar/project_view.rs`, `src/workspace.rs`
(cached_commands field), `src/app/runtime.rs` (tick refresh),
`src/app/input/sidebar.rs`, `src/app/input/mouse.rs`, this file.

- [x] G1: COMMANDS band renders n/m = tagged-panes-alive / declared-selected,
  with running items marked; pinned by tests on a mixed fixture (declared
  commands on the worktree, one tagged pane).
  CHECK: cargo nextest run -E 'test(/commands_section|section_commands|commands_band/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( \(\d+ leaky\))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.017s] 3 tests run: 3 passed, 3998 skipped

- [x] G2: project selection narrows the band (sections.commands subset);
  undeclared projects render no band (rule 5).
  CHECK: cargo nextest run -E 'test(/commands_section|commands_narrow|commands_absent/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( \(\d+ leaky\))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.016s] 3 tests run: 3 passed, 3998 skipped

- [x] G3: clicking a command row launches it (PendingBoraCommand dispatched
  with the command's label, like the deleted band's launch path).
  CHECK: cargo nextest run -E 'test(/section_item.*launch|launch.*command|command.*click/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( \(\d+ leaky\))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.015s] 3 tests run: 3 passed, 3998 skipped

- [x] G4: Programs band gone: no references to sidebar_program_row_count,
  sidebar_programs_band_rect, render_programs_section, sidebar_programs_rect
  anywhere in src/.
  CHECK: grep -rn "sidebar_program_row_count\|sidebar_programs_band_rect\|render_programs_section\|sidebar_programs_rect" src/ | wc -l | tr -d ' '
  EXPECT: 0
  EVIDENCE: 0

- [x] G5: lockstep contract intact and render stays pure (band reads
  Workspace.cached_commands refreshed on the tick — no loader calls from the
  entry builder).
  CHECK: cargo nextest run -E 'test(/lockstep|entry_row_height/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( \(\d+ leaky\))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.013s] 6 tests run: 6 passed, 3995 skipped

- [x] G6: full suite green after the change (lead-run).
  CHECK: cargo nextest run --no-fail-fast 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( \(\d+ leaky\))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  23.420s] 4000 tests run: 4000 passed, 1 skipped
