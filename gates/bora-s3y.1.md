# Gates: bora-s3y.1 — todo + scratchpad stores

Scope: two new project-scoped stores modeled on the append+cursor+event
pattern of `src/persist/channels.rs`. Todos carry title, state (open/done),
blockers, assignee (agent), origin. Scratchpads are named markdown docs with
sections, append_section, and find. Owned paths: new files
`src/persist/todos.rs` and `src/persist/scratchpads.rs`, the `mod` declarations
in `src/persist.rs`, this file. NOT owned: socket verbs, events, MCP, sidebar
(bora-s3y.2 / .3).

- [x] G1: todo store persists and reloads: create todos with all five fields,
  reload from disk, assert full round-trip equality.
  CHECK: cargo nextest run -E 'test(/todo/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.016s] 9 tests run: 9 passed, 3946 skipped

- [x] G2: scratchpad store persists and reloads named docs; append_section adds
  a section; find returns section hits (title or body match), with tests.
  CHECK: cargo nextest run -E 'test(/scratchpad/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.015s] 8 tests run: 8 passed, 3947 skipped

- [x] G3: cursor semantics match channels: append returns/advances a cursor, a
  reader replaying from cursor 0 sees every record in order, a reader at the
  tip sees only new appends — same contract shape as
  `src/persist/channels.rs`, pinned by tests.
  CHECK: cargo nextest run -E 'test(/cursor|replay|append/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.226s] 120 tests run: 120 passed, 3835 skipped

- [x] G4: blockers are queryable: a todo blocked by an open todo is excluded
  from an actionable listing and included once its blocker completes (the
  store-level primitive bora-s3y.2's verbs will expose).
  CHECK: cargo nextest run -E 'test(/blocker|actionable/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.169s] 13 tests run: 13 passed, 3942 skipped

- [x] G5: full suite green after the change (lead-run).
  CHECK: cargo nextest run 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  23.049s] 3954 tests run: 3954 passed, 1 skipped
