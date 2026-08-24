# Gates: bora-i1r.2 — CHECKS section, n/m, configurable refresh

Scope: `checks_counts` beside `checks_rollup`; worktree-level CHECKS section
(failing checks as rows, PR #N on worktree row, n/m = passing/total, error
visible); `CHECKS_REFRESH_INTERVAL` configurable mirroring `[github]
refresh_interval_secs`, documented. Owned paths:
`src/workspace/git/check_status.rs`, `src/ui/sidebar.rs` (CHECKS section only),
`src/config/model.rs`, `src/app/mod.rs` (interval), `src/app/runtime.rs`,
`website/src/data/config-reference.json`, this file. NOT owned: src/app/api.rs
verb surface (s3y.2's), COMMANDS section (55c.2).

- [x] G1: `checks_counts(&[CheckRun]) -> (passing, total)` exists beside
  `checks_rollup`, tested with mixed SUCCESS/FAILURE/pending and legacy
  StatusContext shapes.
  CHECK: cargo nextest run -E 'test(/counts|rollup/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.071s] 20 tests run: 20 passed, 3971 skipped

- [x] G2: CHECKS section renders from the provider outcome: n/m = passing/total,
  failing checks as rows, PR #N on the worktree row, provider error as an
  error row (never silently empty), not-applicable renders no section. Tests
  pin the rows.
  CHECK: cargo nextest run -E 'test(/checks_section|section_checks|checks_row/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.018s] 9 tests run: 9 passed, 3982 skipped

- [x] G3: refresh interval configurable (mirrors `[github]
  refresh_interval_secs`), default stays 30s; config reference documents it.
  CHECK: cargo nextest run -E 'test(/refresh_interval|checks_refresh/)' 2>&1 | tail -3 && grep -c "checks" website/src/data/config-reference.json
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: Summary [   0.018s] 4 tests run: 4 passed, 3987 skipped | 3

- [x] G4: sidebar lockstep contract intact (entry_row_height + visible_count +
  compute_areas + render agree) and any new section rows are height-1 or go
  through `entry_row_height`; lockstep characterization test green.
  CHECK: cargo nextest run -E 'test(/lockstep|entry_row_height/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.013s] 6 tests run: 6 passed, 3985 skipped

- [x] G5: full suite green after the change (lead-run).
  CHECK: cargo nextest run --no-fail-fast 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  24.080s] 3990 tests run: 3990 passed, 1 skipped
