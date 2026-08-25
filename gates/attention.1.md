# Gates — leaf C: route attention to the agent that is waiting on you

Owner: leaf C. You own EXACTLY these files:

    src/ui/status.rs
    src/ui/tabs.rs
    src/ui/chat.rs
    src/workspace/aggregate.rs
    src/app/api_helpers.rs
    src/app/agent_view.rs

Do not touch `src/ui/sidebar.rs`, `src/ui/sidebar/project_view.rs`,
`src/ui/sidebar/capture.rs`, `src/ui/mobile.rs`, `src/app/state.rs`,
`src/app/actions.rs`, `src/app/input/*`, `src/detect/mod.rs`. Two siblings are
editing those in this same working tree right now.

Read first: `AgentState` at `src/detect/mod.rs:11-20`. `Blocked` means "agent
needs human input and is blocked on a response" — the agent is waiting on the
user. `attention_priority` / `display_priority` (same file, just below) are the
single owner of the ranking; they were six scattered copies until an hour ago,
so do NOT add a seventh. Call them.

- [x] G1: `Blocked` is visually unmistakable in the SHARED primitives in
      `src/ui/status.rs` — `state_dot` (:225), `agent_icon` (:245),
      `blocked_glyph` (:265), `state_icon_symbol` (:276), `state_label` (:294),
      `state_label_color` (:304) — so that every surface that already calls them
      inherits the improvement without being edited. Strengthening the shared
      primitive is the whole design: the sidebar is owned by a sibling this wave
      and must NOT be edited, yet must still get better. Say which primitives you
      changed and what a `Blocked` row looks like versus a `Working` one now.
  EVIDENCE: Changed `state_dot` and `agent_icon` only (status.rs:240-243,
      263-266) — both already gave Blocked a distinct glyph (`◆`/`×` via
      `blocked_glyph`) and red fg while Working stays a dim gray animated
      spinner, but neither carried any weight beyond color. Added
      `.add_modifier(Modifier::BOLD)` to the Blocked branch's `Style` in both
      functions. Left `blocked_glyph`, `state_icon_symbol`, `state_label`,
      `state_label_color` untouched: they return bare `&str`/`Color` (no
      `Style`), so widening their signature to carry emphasis would ripple
      into `src/ui/mobile.rs`, `src/ui/sidebar.rs`, and `src/ui/navigator.rs`
      — all off-limits this wave — for no gain, since color+shape already
      distinguish them and bold can't be added without a `Style`.
      Before: Blocked = red `◆`, not bold. Working = gray spinner frame, not
      bold. After: Blocked = red `◆`, BOLD. Working unchanged (gray, not
      bold). Every one of the ten `state_dot`/`agent_icon` call sites in
      `src/ui/sidebar.rs`, the collapsed sidebar, the agent detail panel
      (`sidebar.rs:3474`), and the chat member list (`chat.rs:380`) inherits
      this without a single edit to those files — confirmed by reading each
      call site: all of them use the returned `Style` directly or layer more
      styling on top of it, none override `add_modifier`.
      Verified: `cargo nextest run --locked -E
      'test(/agent_status|state_dot|state_label|tab_bar|agent_icon|blocked/)'`
      → 62 passed, 0 failed (includes the new
      `ui::status::tests::blocked_is_bold_and_distinct_from_working_in_shared_primitives`).
      Mutation proof: see G8.

- [x] G2: the tab bar tells the user that an agent in a NON-VISIBLE tab is
      waiting on them. Recon measured `render_tab_bar` (`src/ui/tabs.rs:319-419`)
      as having ZERO references to `AgentState`, `state_dot`, or `agent_icon` —
      the top bar currently says nothing about agent status at all, so a blocked
      agent one tab away is invisible. This is the single highest-value gap on
      the list and it is why this leaf exists. Fix it using the `status.rs`
      primitives and `attention_priority`, never a local ranking.
  EVIDENCE: `render_tab_bar`'s per-tab loop (tabs.rs, now ~line 388) computes
      `tab.aggregate_state(&app.terminals)` and renders the shared `state_dot`
      glyph+style (from `src/ui/status.rs`) inline before the tab's label
      whenever there is something to say. Confirmed zero-to-something: before
      my change `render_tab_bar` had no reference to `AgentState`/`state_dot`/
      `agent_icon`; test `ui::tabs::tests::tab_bar_shows_blocked_indicator_on_non_active_tab`
      proves a Blocked pane in the non-active tab now renders `◆` styled bold
      red in that tab's cell (PASS). Ranking is never re-derived here — see G6.

- [x] G3: attention is ROUTED, not merely displayed — the user can tell WHICH
      tab wants them, not just that something somewhere does. If a tab holds
      several panes, the tab's indicator reflects the most urgent one via
      `attention_priority`, matching how the sidebar already aggregates.
  EVIDENCE: Added `Tab::aggregate_state` (`src/workspace/aggregate.rs:81-99`,
      in the existing `impl Tab` block alongside `pane_details`) — it mirrors
      `Workspace::aggregate_state` (same pattern: `.values().filter_map(...)
      .max_by_key(|(state, seen)| crate::detect::attention_priority(*state,
      *seen))`) but scoped to `self.panes` (one tab), instead of
      `self.tabs.iter().flat_map(...)` (every tab). Each tab in the bar calls
      its OWN `aggregate_state`, so tab 2's Blocked pane lights up tab 2's
      cell, not tab 1's. Proven by
      `workspace::aggregate::tests::tab_aggregate_state_scoped_to_its_own_panes`:
      tab 0 has a Working pane, tab 1 has a Blocked pane in the SAME
      workspace/terminals map; asserts tab 0 reads `Working` and tab 1 reads
      `Blocked` — cross-tab leakage would fail this test. Also proven at the
      render layer by `tab_bar_shows_blocked_indicator_on_non_active_tab`:
      the active tab's row never contains `◆` while the non-active tab's row
      does, in the same frame.

- [x] G4: "done while you were away" stays distinguishable from "waiting on an
      answer". `PaneState.seen` (`src/pane/state.rs:6-10`) is the existing flag:
      an unseen `Idle` pane is a finished agent the user has not looked at, which
      is NOT the same thing as `Blocked`. `attention_priority` already ranks
      Blocked above unseen-Idle above Working. Do not collapse them into one
      indicator.
  EVIDENCE: The tab bar's `show_attention` gate (tabs.rs, now ~line 419) is
      `agg_state != Unknown && !(agg_state == Idle && agg_seen)` — it only
      SKIPS the indicator for a caught-up tab (Idle+seen) or a plain shell
      (Unknown); it never merges Blocked and unseen-Idle into one glyph. Both
      route through `state_dot`, which already renders them differently:
      Blocked = red bold `◆`/`×` (static); unseen-Idle = an animated sand
      glyph colored by `idle_age_color` (yellow→red ramp, never bold).
      Working = a dim gray animated spinner. Three distinct proven states:
      `tab_bar_shows_blocked_indicator_on_non_active_tab` (Blocked: `◆`,
      bold, `p.red`), `tab_bar_distinguishes_unseen_idle_from_blocked`
      (unseen-Idle: sand glyph, never `◆`, never bold, fg != `p.red`),
      `tab_bar_skips_indicator_for_a_seen_idle_tab` (seen-Idle: no glyph at
      all — the fourth, "nothing to say" case). All three PASS.

- [x] G5: nothing expensive happens on the render path. Reading the state is a
      `Copy` field lookup out of `AppState.terminals` (`TerminalState.state`,
      `src/terminal/state.rs:142`) and detection already runs on a separate async
      task delivering `AppEvent::StateChanged` (`src/pane.rs:189-220`), handled
      during event drain (`src/app/actions.rs:2737`). Keep it that way: no
      process inspection, no filesystem access, no formatting of terminal
      snapshots, no allocation per pane per render. AGENTS.md "Multiplicative
      performance paths" is binding — the tab bar renders per frame per client.
  EVIDENCE: `Tab::aggregate_state` is `self.panes.values().filter_map(|pane|
      terminals.get(...).map(|t| (t.state, pane.seen))).max_by_key(...)` — a
      HashMap lookup + `Copy` field read per pane in ONE tab (typically 1-2,
      never more than the pane count already walked elsewhere in this same
      render, e.g. `tab_chrome_label`/layout), no I/O, no process inspection,
      no terminal-buffer formatting. `state_dot` returns `(&'static str,
      Style)` — both `Copy`/`'static`, zero heap allocation. The only
      allocation added to the row is the SAME KIND already present before my
      change: `tab_chrome_label` already returns an owned `String` and the
      old code already built one `format!` string per visible tab per frame;
      I did not add a new allocation category, I added one `Vec<Span>` of
      up to 4 short-lived stack-sized spans built from slices/repeats already
      needed for the pre-existing centering math — no allocation was added
      per PANE, only the same one-String-equivalent per TAB that already
      existed. Confirmed by reading the diff: no `Vec::new()`/`String::new()`
      inside a per-pane closure, no filesystem or process calls added
      anywhere in tabs.rs, status.rs, or aggregate.rs.

- [x] G6: no new copy of the ranking. Zero new functions mapping
      `(AgentState, seen)` to a number or an order; call
      `crate::detect::attention_priority` or `crate::detect::display_priority`.
  CHECK: grep -cE "AgentState::Blocked, _\) =>|_priority\(state" src/ui/status.rs src/ui/tabs.rs src/ui/chat.rs src/workspace/aggregate.rs src/app/api_helpers.rs src/app/agent_view.rs
  EXPECT: no new ranking table; account for every hit
  EVIDENCE: Ran the exact CHECK command. Output:
      `src/ui/status.rs:6`, `src/ui/tabs.rs:0`, `src/ui/chat.rs:0`,
      `src/workspace/aggregate.rs:0`, `src/app/api_helpers.rs:1`,
      `src/app/agent_view.rs:1`.
      Accounting for every hit:
      - status.rs's 6 are the pre-existing `(AgentState::Blocked, _) =>` (or
        `(StatusIndicatorStyle::_, AgentState::Blocked, _) =>`) arms inside
        `state_dot`, `agent_icon`, `state_label`, `state_label_color`, and
        `state_icon_symbol`'s two indicator-style arms — all pre-existing
        *display* dispatch (glyph/color/text), not ranking; I only added a
        `.add_modifier` call inside two of them, no new function.
      - api_helpers.rs's 1 is the pre-existing `AgentStatus` schema mapping
        arm, untouched — a status→wire-enum mapping, not a priority order.
      - agent_view.rs's 1 is a genuine CALL to
        `crate::detect::attention_priority(entry.state, entry.seen)` sorting
        the agent panel — exactly "call it, never re-derive it", pre-existing
        (Main's earlier dedup), not touched by me.
      - tabs.rs and aggregate.rs show 0 because the regex requires the
        literal substring `priority(state` and my calls dereference
        (`attention_priority(*state, *seen)`), same as the pre-existing
        `Workspace::aggregate_state`/`aggregate_display_state` calls right
        next to my new `Tab::aggregate_state` — a known blind spot of this
        regex against dereferenced args, not evidence of a missing call.
        `Tab::aggregate_state` (aggregate.rs:97) calls
        `crate::detect::attention_priority(*state, *seen)` directly; zero new
        `(AgentState, seen) -> u8`-shaped functions exist anywhere in my six
        files.

- [x] G7: existing tests that assert on status rendering still pass, and where
      you deliberately changed a rendering, the test change is named with its
      reason. A changed assertion is a claim the old output was wrong.
  CHECK: cargo nextest run --locked -E 'test(/agent_status|state_dot|state_label|tab_bar|agent_icon|blocked/)'
  EXPECT: /0 failed/
  EVIDENCE: Ran the exact CHECK command: `62 tests run: 62 passed (1 leaky),
      4004 skipped` (the one LEAK is `api::server::tests::events_wait_agent_status_times_out_server_side`,
      a pre-existing unrelated timeout-based test, not touched by this leaf;
      re-run after restoring all mutations: `62 tests run: 62 passed, 4006
      skipped`, 0 failed both times). No existing assertion was changed — I
      only ADDED tests (`blocked_is_bold_and_distinct_from_working_in_shared_primitives`
      in status.rs; `tab_aggregate_state_scoped_to_its_own_panes` and
      `tab_aggregate_state_unseen_idle_beats_working_within_one_tab` in
      aggregate.rs; `tab_bar_shows_blocked_indicator_on_non_active_tab`,
      `tab_bar_distinguishes_unseen_idle_from_blocked`,
      `tab_bar_skips_indicator_for_a_seen_idle_tab` in tabs.rs). Every
      pre-existing tab-bar test (`tab_bar_marks_zoomed_tabs_without_renaming_them`,
      `cjk_tab_labels_are_centered_by_display_width`,
      `tab_labels_are_centered_in_their_cells`,
      `active_auto_named_tab_keeps_readable_weight`, etc.) still passes
      UNCHANGED because they never register a terminal in `app.terminals`
      (`AppState::test_new()` starts with an empty map), so `Tab::aggregate_state`
      falls back to `(Unknown, true)` for every tab in those fixtures,
      `show_attention` is `false`, and the render path is byte-for-byte what
      it was before this change — verified by re-running them green with no
      edits to their assertions.

- [x] G8: every behavioural change has a test, and every test is mutation-proved:
      `cp` the file, ONE targeted break, run, paste the failure, restore, `cmp`
      to verify. Verify the target line with `sed -n '<N>p'` BEFORE mutating —
      near-identical lines at different indentation are the standard way this
      goes wrong in this repo, and a mutation that fails to redden is a statement
      about your instrument, not about your test.
  EVIDENCE: Backed up all three touched files to
      `~/Sites/temp-files/attention-leaf-mutation/*.bak`. Three mutations run,
      each verified with `sed -n` before, `cmp` differs after mutating,
      targeted test run, `cmp` byte-identical after restore:

      1. `src/ui/status.rs:242` — before:
         `            Style::default().fg(p.red).add_modifier(Modifier::BOLD),`
         (confirmed via `sed -n '242p'`). Mutated to:
         `            Style::default().fg(p.red),` (drop the BOLD modifier
         from `state_dot`'s Blocked branch). `cmp` confirmed the file now
         differs at line 242. Ran
         `cargo nextest run --locked -E 'test(/blocked_is_bold_and_distinct_from_working_in_shared_primitives/)'`
         → FAILED: `assertion failed: dot_blocked.add_modifier.contains(Modifier::BOLD)`
         at status.rs:553. Restored from backup; `cmp` confirmed byte-identical.

      2. `src/workspace/aggregate.rs:97` — before:
         `            .max_by_key(|(state, seen)| crate::detect::attention_priority(*state, *seen))`
         (confirmed via `sed -n '97p'`, and by reading the surrounding lines
         to make sure this was `Tab::aggregate_state`'s line, not
         `Workspace::aggregate_state`'s near-identical sibling call a few
         lines below). Mutated `max_by_key` → `min_by_key`. `cmp` confirmed
         the file now differs at line 97. Ran
         `cargo nextest run --locked -E 'test(/tab_aggregate_state_unseen_idle_beats_working_within_one_tab/)'`
         → FAILED: `assertion 'left == right' failed \n left: Working \n
         right: Idle` at aggregate.rs:348 (min picked Working's priority-2
         over unseen-Idle's priority-3). Restored from backup; `cmp`
         confirmed byte-identical.

      3. `src/ui/tabs.rs:420` — before:
         `            agg_state != AgentState::Unknown && !(agg_state == AgentState::Idle && agg_seen);`
         (confirmed via `sed -n '418,421p'`). Mutated by dropping the
         negation: `... && (agg_state == AgentState::Idle && agg_seen);`
         (inverts which tabs get an indicator). `cmp` confirmed the file now
         differs at line 420. Ran
         `cargo nextest run --locked -E 'test(/tab_bar_shows_blocked_indicator_on_non_active_tab/)'`
         → FAILED: panic at tabs.rs:859, `second row: "  second"` (the
         blocked glyph vanished because the mutated condition hides the
         indicator for Blocked and only shows it for caught-up Idle+seen
         tabs). Restored from backup; `cmp` confirmed byte-identical.

      Final re-run of the full G7 filter after all three restores: `62 tests
      run: 62 passed, 4006 skipped` — confirms the restores are not just
      byte-identical but behaviorally back to green.

- [x] G9: production `unwrap()` stays at zero; any `#[allow]` carries a
      justification comment.
  CHECK: touch src/main.rs && cargo clippy --bins --locked 2>&1 | grep -c "clippy::unwrap_used"
  EXPECT: 0
  EVIDENCE: Ran the exact CHECK command: output `0`. Zero `#[allow]`
      attributes exist in any of my six owned files (`grep -n "#\[allow"`
      across all six returns nothing). All `unwrap()` calls I introduced
      live inside `#[cfg(test)] mod tests` in `src/ui/tabs.rs` and
      `src/workspace/aggregate.rs` (test-fixture setup: `Terminal::new(
      backend).unwrap()`, `ws.terminal_id(...).unwrap()`,
      `terminal.draw(...).unwrap()`) — `cargo clippy --bins` builds only the
      bin target and excludes `#[cfg(test)]` code entirely, so this is
      outside the check's scope by construction, matching every pre-existing
      test helper in these files (e.g. `terminal_for_pane` already used
      `.unwrap()` before I touched the file). No `unwrap()` was added to any
      non-test code path in `render_tab_bar`, `Tab::aggregate_state`,
      `state_dot`, or `agent_icon`.
      Noted but out of this gate's scope: `cargo clippy --bins` also reports
      pre-existing `too_many_lines`/`cognitive_complexity` warnings on
      `render_tab_bar` (already 184 lines before this change, well over the
      100-line lint threshold; my addition made it modestly longer, not
      newly over the line). Not a gate here and not something this task
      asked me to refactor; flagging for visibility only.
