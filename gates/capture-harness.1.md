# Gates — leaf B: deterministic sidebar capture harness

Owner: leaf B. You own EXACTLY ONE file:

    src/ui/sidebar/capture.rs

It already exists with a module doc comment and nothing else, and it is already
declared as `#[cfg(test)] mod capture;` at `src/ui/sidebar.rs:1-2`. Do not edit
`src/ui/sidebar.rs` or anything else — two siblings are editing other files in
this same working tree right now, and one of them is restructuring
`src/ui/sidebar.rs` heavily.

Because `capture` is a CHILD module of `ui::sidebar`, it can call
`sidebar`'s private items, including `render_workspace_list`
(`src/ui/sidebar.rs:2545`). That is why the module lives here instead of in
`tests/`, and it is why no visibility needs widening for this work.

- [x] G1: `capture.rs` exposes a function that renders the sidebar's workspace
      list from a given `AppState` at a caller-chosen fixed width and height and
      returns a `String`. Reuse the established recipe:
      `AppState::test_new()` (`src/app/state.rs:2581`),
      `TerminalRuntimeRegistry::new()`, `TestBackend::new(w, h)` inside
      `Terminal::draw`. There are already 9 tests calling `render_workspace_list`
      this way in `src/ui/sidebar.rs` (lines 4619, 4645, 4693, 4730, 4772, 4829,
      4877, 4919, 4963) — follow that pattern, do not invent a new one.
  EVIDENCE: `fn capture_sidebar(app: &AppState, width: u16, height: u16) -> String`
  at `src/ui/sidebar/capture.rs:132`, following the exact 9-test recipe
  (`TerminalRuntimeRegistry::new()`, `Terminal::new(TestBackend::new(width,
  height))`, `terminal.draw(|frame| render_workspace_list(...))`). Compiles and
  runs — see G4/G6 test output below.

- [x] G2: the capture includes STYLE, not only text. ratatui is 0.30.2 and
      `ratatui-core` 0.1.2 exposes `Cell::fg`, `Cell::bg`, `Cell::modifier` as
      public fields plus `Cell::symbol()` and `Cell::style()`. All four existing
      text-flatteners in this repo (`src/ui.rs:1470`, `src/ui/tabs.rs:511`,
      `src/ui/sidebar.rs:3613`, and an inline closure in
      `src/ui/sidebar/project_view.rs:3134`) throw style away, which is exactly
      why they are useless for judging a visual change. Yours must not.
  EVIDENCE: `serialize_buffer`/`style_runs`/`format_span`
  (`src/ui/sidebar/capture.rs:143-182`) read `Cell::fg`/`Cell::bg`/`Cell::modifier`
  directly and emit a `start..end=fg:…,bg:…,mod:…` span per row. Real captured
  output shows it firing on live theme colors, e.g. row 08 (the active "main"
  workspace row):
  `row 08 style 0..1=fg:Reset,bg:Rgb(30, 30, 46),mod:NONE 1..2=fg:Rgb(243, 139, 168),bg:Rgb(30, 30, 46),mod:BOLD 2..3=fg:Reset,bg:Rgb(30, 30, 46),mod:NONE 3..7=fg:Rgb(205, 214, 244),bg:Rgb(30, 30, 46),mod:BOLD 7..16=fg:Rgb(203, 166, 247),bg:Rgb(30, 30, 46),mod:NONE 16..56=fg:Reset,bg:Rgb(30, 30, 46),mod:NONE`
  — fg, bg, AND modifier (BOLD on the Blocked glyph and the active-row name) all
  present, not just text. Mutation-proved in G8.

- [x] G3: the serialization is DETERMINISTIC and diff-friendly. Same fixture and
      same dimensions produce byte-identical output across runs, and a
      one-attribute change (say one row going bold) produces a SMALL diff rather
      than reflowing every line. Choose a format that a human and a `diff` can
      both read, and state the choice and its reason in the module doc comment.
  EVIDENCE: format + rationale documented in the module doc comment
  (`src/ui/sidebar/capture.rs:13-43`, "## Serialization format"): two plain-text
  lines per row (`text`, then a run-length-encoded `style` span line), chosen
  specifically so a single-row attribute change touches exactly one line out of
  `2 * height`. Proved, not just asserted: test
  `one_attribute_change_produces_a_small_diff`
  (`src/ui/sidebar/capture.rs:459-490`) builds a 10x2 buffer, flips one cell's
  `Modifier::BOLD`, and asserts only line index 2 (row 0's style line) differs
  between the two captures — `cargo nextest run -E
  'test(/one_attribute_change_produces_a_small_diff/)'` passes (see full run
  below). Determinism itself is G4's evidence.

- [x] G4: determinism is asserted, not asserted-by-hand. A test calls the capture
      twice on the same fixture and asserts byte equality. This is the property
      the whole instrument rests on: a capture that varies run to run cannot
      measure anything.
  EVIDENCE: `capture_is_byte_identical_across_two_calls_on_the_same_state`
  (`src/ui/sidebar/capture.rs:388-398`) builds the fixture once, captures twice,
  `assert_eq!`s. A second, stronger test
  `capture_is_byte_identical_across_two_independent_fixture_builds`
  (`:412-427`) rebuilds the fixture from scratch a second time (fresh on-disk
  fake checkout, fresh `ProjectsStore::load()`, fresh `AppState`) and compares
  that independently-built capture too — the actual cross-commit property this
  instrument exists for. Both pass:
  ```
  cargo nextest run --locked -E 'test(/ui::sidebar::capture/)'
  PASS ui::sidebar::capture::tests::capture_is_byte_identical_across_two_calls_on_the_same_state
  PASS ui::sidebar::capture::tests::capture_is_byte_identical_across_two_independent_fixture_builds
       Summary [0.056s] 5 tests run: 5 passed, 4063 skipped
  ```
  Mutation-proved in G8.

- [x] G5: a fixture builder produces a MULTI-workspace, multi-band sidebar —
      several workspaces under one project, at least one worktree row, at least
      one band with items, and at least one agent in each of the interesting
      states. Recon found no multi-workspace sidebar fixture exists anywhere in
      the repo today, so this is genuinely new. The closest template to adapt is
      `workspace_list_lockstep_pull_requests_agree_across_passes` at
      `src/ui/sidebar/project_view.rs:2998-3147` — read it first.
      Use `AppState::ensure_test_terminals()` (`src/app/state.rs:2817`) so you do
      not hand-build `TerminalState` entries.
  EVIDENCE: `multi_workspace_fixture()` (`src/ui/sidebar/capture.rs:292-377`,
  doc at `:270-291`) builds 5 workspaces (`main`, `feature-x`, `feature-y`,
  `cleanup`, `scratch`) under one declared project "Bora" — each gets its own
  `WorktreeRow`; `main` also carries a non-empty COMMANDS band (1 declared
  item, "dev") and CHECKS band (2 checks, 1 failing — "clippy"); the 5
  workspaces cover all four `AgentState` variants plus the `Idle` seen/unseen
  split (`Blocked`, `Idle`+unseen, `Working`, `Idle`+seen, `Unknown`/no agent).
  Confirmed against the REAL rendered capture (`print_sidebar_capture` output,
  pasted in full in the final report — see below for the excerpt): rows show
  `▾ ⬢ Bora … 5/5`, `▾ main #42`, `≡ COMMANDS … 0/1` / `· dev`, `✓ CHECKS … 1/2`
  / `✗ clippy`, `◆ main`, `▾ feature/x` / `⠁ feature-x`, `▾ feature/y` / `⠋
  feature-y`, `▾ cleanup` / `○ cleanup`, `▾ scratch` / `◰ scratch` — five
  distinct glyphs for five distinct states. Sanity-pinned by
  `fixture_capture_shows_every_worktree_and_every_agent_state`
  (`:442-457`), passing (see G4's run output). Reused
  `AppState::ensure_test_terminals()` exactly as directed
  (`:346` `app.ensure_test_terminals();`) — no hand-built `TerminalState`.
  Getting a non-empty COMMANDS/CHECKS band required a *declared* project
  (`project_view`'s band builders return early for the orphans group,
  regardless of `WorkspaceListEntry`'s internal representation — see G7 for why
  and how that stays deterministic); the module doc at `:270-291` explains why
  that was structurally required, not a scope add.

- [x] G6: the capture is OBTAINABLE as text by a human running one command, not
      only reachable from inside an assertion. A reviewer must be able to see the
      rendering. Print it from a test and give the exact command that shows it.
  CHECK: the command you document, run verbatim
  EXPECT: the sidebar rendering, visible as text
  EVIDENCE: command (documented at `src/ui/sidebar/capture.rs:431-432` above
  `print_sidebar_capture`):
  `cargo test --locked ui::sidebar::capture::tests::print_sidebar_capture -- --exact --nocapture`
  Real output (first 17 of 40 rows; full 40-row capture was produced and is
  identical on every run per G4):
  ```
  bora sidebar capture 56x40
  row 00 text  | spaces                                          project|
  row 00 style 0..7=fg:Rgb(108, 112, 134),bg:Reset,mod:BOLD 7..49=default 49..56=fg:Rgb(108, 112, 134),bg:Reset,mod:BOLD
  row 01 text  |                                                        |
  row 01 style 0..56=default
  row 02 text  |▾ ⬢ Bora                                             5/5|
  row 02 style 0..1=fg:Rgb(137, 180, 250),bg:Reset,mod:NONE 1..2=default 2..3=fg:Rgb(203, 166, 247),bg:Reset,mod:NONE 3..4=default 4..8=fg:Rgb(108, 112, 134),bg:Reset,mod:BOLD 8..52=default 52..56=fg:Rgb(108, 112, 134),bg:Reset,mod:NONE
  row 03 text  |  ▾ main #42                                            |
  row 04 text  |    ≡ COMMANDS ───────────────────────────────────── 0/1|
  row 05 text  |      · dev                                             |
  row 06 text  |    ✓ CHECKS ─────────────────────────────────────── 1/2|
  row 07 text  |      ✗ clippy                                          |
  row 08 text  | ◆ main @wfix1p1                                        |
  row 09 text  |  ▾ feature/x                                           |
  row 10 text  | ⠁ feature-x @wfix2p1                                   |
  row 11 text  |  ▾ feature/y                                           |
  row 12 text  | ⠋ feature-y @wfix3p1                                   |
  row 13 text  |  ▾ cleanup                                             |
  row 14 text  | ○ cleanup @wfix4p1                                     |
  row 15 text  |  ▾ scratch                                             |
  row 16 text  | ◰ scratch                                              |
  ```
  (style lines omitted here for brevity, present in the actual output for
  every row — see G2 for a sample.)

- [x] G7: the capture is STABLE ACROSS COMMITS by construction — it depends only
      on the fixture and the dimensions you pass, never on wall-clock time, real
      git state, the developer's home directory, environment variables,
      filesystem contents, HashMap iteration order, or anything else that differs
      between two checkouts of this repo. This is the gate that makes the
      instrument worth building: it will be used to render the SAME fixture on
      two different commits and compare. Go through each of those sources and say
      how you excluded it. If any row's content derives from a `HashMap`
      iteration, that is a real determinism bug and G4 will not catch it, since
      a single process tends to reproduce its own iteration order.
  EVIDENCE: full source-by-source trace is written into the module doc comment
  (`src/ui/sidebar/capture.rs:45-105`, "## Determinism"), summary:
  - **Wall-clock**: `capture_sidebar` reads no clock. `TerminalId::alloc()`
    mixes in `SystemTime::now()`, but that value never reaches rendered text
    (badges print workspace id + pane number, never the raw `TerminalId`) —
    traced, not assumed.
  - **Real git state / filesystem contents**: every `GitSpaceMetadata` on every
    fixture workspace is a hand-built struct literal (same pattern
    `sidebar.rs`'s own `git_space_member` helper uses) — zero disk reads drive
    what renders. The one real disk touch, `FakeGitCheckout` +
    `persist::projects::ProjectsStore::load()`, exists only because
    `project_view`'s COMMANDS/CHECKS band builders return early for any
    unmatched-to-a-declared-project workspace and `ProjectsStore` has no
    in-memory constructor for a declared project (`load()` or `empty()` only)
    — documented in `multi_workspace_fixture`'s doc (`:270-291`). Its content is
    100% fixture-written and fixed every run; `repo_identity` (what actually
    renders on the WorktreeRow) comes from a fixed `origin` URL this file
    writes, never the checkout's absolute path.
  - **`$HOME`/env vars**: `IsolatedDirs` (the repo's existing sanctioned
    mechanism) redirects `XDG_CONFIG_HOME`/`XDG_STATE_HOME` for the fixture's
    lifetime; `Workspace::identity_cwd` (which `Workspace::test_new` otherwise
    defaults to real `current_dir()`) is force-set on every fixture workspace.
  - **`HashMap` iteration order — real bug found, not fixable here**:
    `Workspace::aggregate_state`/`aggregate_display_state`
    (`src/workspace/aggregate.rs`, NOT owned by this leaf) fold
    `tab.panes.values()` — a `HashMap` — via `max_by_key`, which resolves a tie
    (two panes whose `attention_priority` is numerically equal, e.g. two
    `Working` panes differing only in `seen`) by returning the iteration
    order's LAST maximal element. Stable within one process, not across two —
    exactly the trap the gate names, and G4's same-process double-capture
    would NOT have caught it. Reported to the lead (see final report) rather
    than patched, since it lives in a file outside this leaf's ownership. The
    fixture sidesteps it structurally: every fixture workspace has exactly one
    pane, so the fold never has more than one element to choose from — not
    reachable from this capture.
  - **Separately found**: `Workspace::id` defaults from a process-global
    `AtomicU64` counter (`generate_workspace_id`), which is execution-order-
    dependent, not `HashMap`-based but the same shape of bug. It DOES reach
    rendered text (the `@w<id>p1` badges visible in G6's output). Fixed in the
    fixture by overriding `ws.id` explicitly for every workspace
    (`fixture_workspace`, `:238-249`) — visible in the real output as the fixed
    `wfix1`..`wfix5` ids rather than counter-derived ones.
  Confirmed by test, not just argued: `capture_is_byte_identical_across_two_independent_fixture_builds`
  rebuilds the whole pipeline (fresh checkout, fresh `ProjectsStore::load()`,
  fresh `AppState`, including a fresh counter state for anything NOT
  overridden) a second time and passes — the closest thing to an actual
  cross-build check this leaf can run alone.

- [x] G8: mutation proof for the determinism test and the style capture. Break
      each deliberately, one at a time, show the failure, restore, `cmp` to
      verify the restore. Verify the line you are about to mutate with
      `sed -n '<N>p'` before mutating; near-identical lines at different
      indentation are the standard way this goes wrong in this repo.
  EVIDENCE:
  **Mutation 1 — determinism (`capture_sidebar`, line 140).**
  `sed -n '140p' src/ui/sidebar/capture.rs` confirmed:
  `    serialize_buffer(terminal.backend().buffer(), width, height)`
  Changed to append a wall-clock nanosecond value to the output:
  ```
  let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
  format!("{}{}", serialize_buffer(terminal.backend().buffer(), width, height), nanos)
  ```
  `cmp` confirmed the file differs (`differ: char 8106, line 140`). Ran
  `cargo nextest run -E 'test(/capture_is_byte_identical_across_two_calls_on_the_same_state/)'`:
  ```
  FAIL ui::sidebar::capture::tests::capture_is_byte_identical_across_two_calls_on_the_same_state
  panicked at src/ui/sidebar/capture.rs:394:9:
  assertion `left == right` failed: capture must be byte-identical across two calls on the same AppState …
  Summary [0.022s] 1 test run: 0 passed, 1 failed, 4065 skipped
  ```
  Restored from backup; `cmp` confirmed byte-identical restore (exit 0, no output).

  **Mutation 2 — style capture (`same_style`, line 170).**
  `sed -n '169,171p'` confirmed:
  ```
  fn same_style(a: &Cell, b: &Cell) -> bool {
      a.fg == b.fg && a.bg == b.bg && a.modifier == b.modifier
  }
  ```
  Changed line 170 to drop the modifier comparison:
  `    a.fg == b.fg && a.bg == b.bg`
  `cmp` confirmed the file differs (`differ: char 9199, line 170`). Ran
  `cargo nextest run -E 'test(/one_attribute_change_produces_a_small_diff/)'`:
  ```
  FAIL ui::sidebar::capture::tests::one_attribute_change_produces_a_small_diff
  panicked at src/ui/sidebar/capture.rs:484:9:
  assertion `left == right` failed: only row 0's style line … should differ for a single-cell modifier change: …
    left: []
   right: [2]
  Summary [0.012s] 1 test run: 0 passed, 1 failed, 4067 skipped
  ```
  (Empty `left` proves the mutation made the row-0 modifier change invisible to
  the serializer — exactly the defect class G2 exists to catch.) Restored from
  backup; `cmp` confirmed byte-identical restore (exit 0, no output).

  Post-restore, full suite re-confirmed green:
  `cargo nextest run --locked -E 'test(/ui::sidebar::capture/)'` →
  `5 tests run: 5 passed, 4063 skipped`.

- [x] G9: no production code changes. This is a test-only instrument; everything
      you write is behind the existing `#[cfg(test)]` gate.
  CHECK: git diff --stat
  EXPECT: /capture.rs/ and no other file
  EVIDENCE: this working tree has two other leaves actively editing files
  concurrently (confirmed live by the lead's IRC warning about leaf A
  restructuring `sidebar.rs`/`project_view.rs`), so a tree-wide `git diff
  --stat` now legitimately shows other leaves' files too — none of them touched
  by this leaf. Scoped to what this leaf owns:
  `git diff --stat -- src/ui/sidebar/capture.rs` →
  ```
  src/ui/sidebar/capture.rs | 487 ++++++++++++++++++++++++++++++++++++++++++++++
  1 file changed, 487 insertions(+)
  ```
  0 deletions confirms the original module doc comment was extended, not
  replaced destructively. All added lines sit under the file's existing
  `#[cfg(test)] mod capture;` gate (`src/ui/sidebar.rs:1-2`, untouched by this
  leaf) — no production code path executes any of it.
