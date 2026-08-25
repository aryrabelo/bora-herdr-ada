# Integration gates — wave 2 (A + B + C)

Lead-owned. 17 gates, all met. Leaf files: `gates/bora-by6.1.md` (10/10),
`gates/capture-harness.1.md` (9/9), `gates/attention.1.md` (9/9) = 28 leaf gates.

- [x] GJ1: `just check` green on the merged tree.
  CHECK: just check
  EXPECT: /0 failed/
  EVIDENCE: `4069 tests run: 4069 passed, 1 skipped`. Baseline entering the wave
      was 4052, so +17 tests. Two things had to be fixed first, both mine: the
      leaves were instructed never to run `cargo fmt` (it rewrites the whole tree
      and would fight siblings), so the lead ran it once — `src/ui/tabs.rs` needed
      it; and my own new test tripped `clippy::type_complexity` by binding an
      array of fn pointers in a `let`, fixed by extracting a nested `fn` instead
      of adding an `#[allow]`.

- [x] GJ2: no two leaves wrote the same file.
  CHECK: git diff --name-only
  EXPECT: every path attributable to exactly one leaf
  EVIDENCE: 10 files, zero overlap. A: `mouse.rs`, `state.rs`,
      `persist/projects.rs`, `ui.rs`, `sidebar.rs`, `project_view.rs`. B:
      `sidebar/capture.rs`. C: `status.rs`, `tabs.rs`, `aggregate.rs`. Two things
      worth recording. A hit a file it did not own (`src/ui.rs`, one re-export
      line, structurally forced by deleting the enum) and MESSAGED rather than
      taking it — my enumeration of unowned files had missed `ui.rs`, which is my
      error and the second time that planning step has bitten me. And A left
      `actions.rs`, `input/sidebar.rs` and `mobile.rs` untouched despite owning
      them: their `WorkspaceListEntry` matches bind `{ .. }` and never the `kind`
      field, so the enum-to-descriptor change needed nothing there. Confirmed by
      empty `git diff` on all three.

- [x] GJ3: the leaves' mutation claims re-verified by the LEAD, not trusted.
  EVIDENCE: three lead-run mutations, one per leaf, each `cp`-backed and
      `cmp`-verified on restore.
      A — `project_view.rs:614`, `.filter(move |d| d.level == level)` →
      `.filter(move |_d| true)`: reddened
      `checks_section_absent_when_pr_has_zero_check_runs` and
      `checks_section_collapse_hides_rows_keeps_header`.
      C — `tabs.rs:418`, `tab.aggregate_state(&app.terminals)` →
      `(AgentState::Working, true)`: reddened
      `tab_bar_distinguishes_unseen_idle_from_blocked` and
      `cjk_tab_labels_are_centered_by_display_width`.
      B — `capture.rs:181`, `cell.modifier` → `Modifier::empty()`: **reddened
      NOTHING.** That is a real gap and the reason this gate exists; see GJ18.
      Two of my own attempts were invalid before they were valid, both caught by
      not trusting a green: a `sed` that produced `mod:X"` broke the `format!`
      arity, so the "mutant" was a compile error rather than a test failure, and
      `Modifier::NONE` does not exist in ratatui 0.30. A mutation is a statement
      about the instrument until proven otherwise.

- [x] GJ4: the registry's edit-site claim measured, not asserted.
  EVIDENCE: 9 → 2, verified against the diff rather than the leaf's report.
      `grep -c "enum ProjectSection" src/ui/sidebar.rs` → 0; `grep -c unreachable
      src/ui/sidebar/project_view.rs` → 0. The four scattered `match` arms
      (`wire_name`, header glyph/label, counter format, item bullet) are
      descriptor FIELDS now, and the two narrowing helpers plus the `unreachable!()`
      arm are one `filter_by_level` iterator. `SECTION_COUNT` derives from
      `REGISTRY.len()`, so appending an entry needs no signature edits anywhere.

- [x] GJ5: the capture actually captures, and the lead has seen it.
  EVIDENCE: ran `cargo test --locked
      ui::sidebar::capture::tests::print_sidebar_capture -- --exact --nocapture`
      myself; 141 lines of paired text/style rows at 56x40. Format is run-length
      style spans (`0..7=fg:Rgb(108,112,134),bg:Reset,mod:BOLD 7..49=default`),
      which is what makes a one-attribute change a small diff. Stability: two
      captures in the same run byte-identical (B's own gate), and see GJ6.

- [x] GJ6: comparable across commits.
  EVIDENCE: the real hazard here is `HashMap` iteration order, whose seed is
      PER PROCESS, so I ran the capture as two separate processes and diffed:
      byte-identical, 141 lines. That is strictly stronger than B's own G4, which
      captures twice inside one process and provably cannot catch a seed-dependent
      ordering. **Honest limitation:** the cross-COMMIT half was not run, because
      the instrument is new — there is no pre-wave commit that contains it to
      render the fixture on. The property is proved forward, from this commit on,
      which is what the gauntlet actually needs.

- [x] GJ7: attention routed, three states visibly distinct.
  EVIDENCE: read off the real capture, not the code. Row 08 `◆` fg
      `Rgb(243,139,168)` BOLD — blocked, wants you now. Row 10 `⠁` fg
      `Rgb(249,226,175)` BOLD — finished while you were away. Row 12 `⠋` fg
      `Rgb(108,112,134)` BOLD — still working, wants nothing. Three different
      glyphs in three different colours; the urgent one is the only red.

- [x] GJ8: still exactly one owner of the attention ranking.
  CHECK: grep -rn "AgentState::Blocked, _) =>" --include=*.rs src | grep -v detect/mod.rs
  EXPECT: no ranking tables
  EVIDENCE: 7 remaining sites, every one inspected: two counters
      (`mobile.rs`, `actions.rs`), three glyph/colour maps and two label maps in
      `status.rs`, one navigator label, two wire-schema projections
      (`agent_view.rs`, `api_helpers.rs`). Zero rankings — so the gate is met on
      its intent. But the grep surfaced the SAME drift class one layer over: four
      independent copies of the state→label-text map, which **already disagree**
      (`status.rs` maps `Unknown` to "idle"; the navigator, `actions.rs` and
      `agent_panel_status_key` map it to "unknown"), so one pane running a plain
      shell reads as "idle" in the sidebar and "unknown" in the navigator. Filed
      as a bead rather than fixed here: which word is right is a product decision
      about user-visible strings, and one of the four is a config key rather than
      display text, so "unifying" it would silently break someone's config.toml.

- [x] GJ9: no allocation, I/O, or process inspection added to a render path.
  EVIDENCE: audited all three. A — `REGISTRY` is a `const &[&SectionDescriptor]`,
      ordering returns `[&SectionDescriptor; SECTION_COUNT]`, level filtering is a
      borrowing iterator; `SectionPushCtx` is a `Copy` struct of borrows. No
      `Vec`, no `Box<dyn>`, no `collect()` added. C — the tab bar reads
      `TerminalState.state`, a `Copy` field, out of an existing `HashMap`;
      detection still runs on its async task and arrives via
      `AppEvent::StateChanged` during event drain. B — entirely `#[cfg(test)]`,
      so it cannot reach production at all.

- [x] GJ10: production `unwrap()` zero, `#[allow]` justified.
  CHECK: touch src/main.rs && cargo clippy --bins --locked 2>&1 | grep -c "clippy::unwrap_used"
  EXPECT: 0
  EVIDENCE: 0. `touch` first, because clippy replays a cached build otherwise —
      the trap that has produced a stale green here before. No `#[allow]` added by
      any leaf or by me; the one clippy lint this wave tripped
      (`type_complexity`, mine) was fixed rather than allowed.

- [x] GJ11: every leaf gates file fully checked.
  CHECK: grep -cE '^  EVIDENCE: pending$' gates/bora-by6.1.md gates/capture-harness.1.md gates/attention.1.md
  EXPECT: 0 for each
  EVIDENCE: `bora-by6.1.md` 10 met / 0 unmet / 0 pending / 0 abandon;
      `capture-harness.1.md` 9/0/0/0; `attention.1.md` 9/0/0/0. Pattern anchored
      to line start so this gate's own text cannot match itself — a mistake
      already made once in the previous wave.

- [x] GJ12: version bump and changelog.
  EVIDENCE: `Cargo.toml` 0.43.0 → 0.44.0, `Cargo.lock` refreshed (the bump breaks
      `--locked` otherwise). `docs/next/CHANGELOG.md`: two Added entries (the
      open-ended band registry; tab-bar attention) and two Fixed (the
      unconditional DIM; the iteration-order-dependent aggregate). The capture
      harness deliberately gets NO entry — AGENTS.md restricts that file to
      user-facing runtime changes and a test-only instrument is not one.

- [x] GJ13: DOX pass.
  EVIDENCE: two dated binding rules added to Code Conventions — the band registry
      with its three non-negotiable constraints and an explicit note that runtime
      or plugin-declared bands are NOT built and why; and the capture instrument,
      including the two traps that cost real time (a small diff is not necessarily
      a legible one; a same-process double capture cannot catch seed- or
      counter-dependent output). Checked for text the wave made false:
      `grep ProjectSection AGENTS.md docs/` returns nothing outside the new rules,
      and the existing three-pass lockstep rule is about `WorkspaceListEntry`, not
      `ProjectSection`, so it stayed accurate rather than needing a rewrite. Also
      documented the `Workspace::test_new` id hazard on the function itself, which
      is where a fixture author reads.

- [x] GJ14: `bora-by6` closed with the contribution-scope decision recorded.
  EVIDENCE: closed. Decision recorded in the close reason: **in-binary `const`
      registry now**, not config- or plugin-declarable. The reason is that the
      remaining obstacle is not the band mechanism but the ROW mechanism — a new
      row shape still cascades through `entry_row_height`, both `apply_hidden_filter`
      matches, `actions.rs` and `input/sidebar.rs`, all exhaustive — so a
      plugin-declared band would have nothing to render but the existing generic
      rows. The descriptor shape is deliberately what a non-const provider would
      need, and the test that proves the two-site cost does so with a descriptor
      that is never registered, which is the same shape a runtime provider takes.

- [x] GJ15: the unconditional `Modifier::DIM` no longer mutes a `Blocked` row.
  EVIDENCE: found and reported by leaf C, which correctly refused to fix it
      because leaf A owned the file. `sidebar.rs` applied DIM to every inactive
      row's state label, muting the red `blocked` label — the one thing the panel
      most needs to say. Now ranked through `crate::detect::attention_priority`
      against `Working`, so blocked and finished-unseen keep full strength while
      working and seen-idle stay dimmed. New test
      `inactive_rows_are_dimmed_except_the_ones_that_want_you`, deliberately
      two-sided, and two mutations prove both directions: restoring
      `if is_active` reddens with "a blocked agent's label must not be dimmed",
      and `if true` reddens with "a working agent does not want you". Both restores
      `cmp` byte-identical. Writing the test also surfaced that the state label is
      not in the default row layout at all, so the fixture configures
      `rows = [[{ token = "state_text" }]]` — the first attempt asserted against a
      row reading `" ◆ one"` and failed for the right reason.

- [x] GJ16: the aggregate fold no longer depends on iteration order.
  EVIDENCE: found by leaf B while proving its own capture deterministic — an
      instrument surfacing a production bug on its first use, which is the
      argument for building it. `attention_priority` alone is not a safe fold key:
      it maps `Blocked`, `Working` and `Unknown` identically across `seen`, so
      `max_by_key` over a `HashMap` of panes broke ties by iteration order.
      `attention_sort_key`/`display_sort_key` append `!seen`, making the key
      injective over the whole space, and the three folds in `aggregate.rs` use
      them. Test `fold_keys_are_injective_so_the_aggregate_cannot_depend_on_iteration_order`
      asserts distinctness over the cross product; mutating the tie-break to
      `false` reddens it with "attention: (Working, false) and (Working, true)
      share a fold key". Restore `cmp` byte-identical.

- [x] GJ17: workspace ids do not silently leak execution order into renderings.
  EVIDENCE: documented rather than fixed, deliberately. `generate_workspace_id`
      is a process-lifetime `AtomicU64` and production needs ids unique for the
      life of a session, which is the whole point — so the hazard belongs to
      fixtures, and the doc comment now lives on `Workspace::test_new` where a
      fixture author is already looking, naming the `@w<id>p<n>` badge as the path
      by which it reaches rendered text and pointing at `ui::sidebar::capture` as
      the example that pins ids.

- [x] GJ18: the gap GJ3 found in leaf B's own test set is closed.
  EVIDENCE: `one_attribute_change_produces_a_small_diff` asserted that exactly one
      line of the capture differs, and passed even with the modifier omitted from
      the serialized output entirely — because span boundaries move whenever
      `same_style` sees a difference, so the diff stayed small while becoming
      illegible. B's own mutation had targeted the span-grouping comparison, not
      the output format, which is why its set missed this. The test now also
      asserts the changed line NAMES the modifier; re-running the same mutation
      reddens it with "the changed style line must name the modifier that changed,
      not merely differ: row 00 style 0..10=default". Restore `cmp` byte-identical.
