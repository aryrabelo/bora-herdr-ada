# Gates: bora-55c.2 — commands run as tagged panes

Scope: the Pane arm of `execute_bora_command` stores the originating command's
label on the spawned pane (one Option on PaneState); shell mode stays
fire-and-forget and uncounted. Owned paths: `src/app/mod.rs` (execute path),
the PaneState definition site, this file. NOT owned: sidebar (55c.3), config
loading (55c.1 landed).

- [x] G1: launching a command in Pane mode spawns a pane whose state carries
  the command label; test fails if the field is dropped.
  CHECK: cargo nextest run -E 'test(/command.*pane|pane.*command|tagged|label/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.397s] 57 tests run: 57 passed, 3936 skipped
    branch of execute_bora_command calls tag_launched_command_pane_label
    (src/app/mod.rs:2589-2591), which sets the focused pane's command_label
    (src/app/mod.rs:2597-2613; field + None default src/pane/state.rs:13-25).
    The label flows through PendingBoraCommand.label (src/app/state.rs:1723)
    set at both modal dispatch sites (src/app/input/modal.rs:1167, 1824).
    Test `pane_command_label_tags_the_launched_pane`
    (src/app/mod.rs:8057-8083) pins untagged-before / tagged-after.

- [x] G2: shell-mode commands remain untagged/uncounted (fire-and-forget
  semantics unchanged — no pane, no label).
  CHECK: cargo nextest run -E 'test(/shell|fire_and_forget|uncounted/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   2.767s] 54 tests run: 54 passed, 3939 skipped
    (src/app/mod.rs:2552-2576 — /bin/sh -lc, Stdio::null, Child dropped, no
    Pane), so a shell run never tags a pane. Any non-command PaneState starts
    untagged via PaneState::new's command_label: None
    (src/pane/state.rs:13-25). Regression test
    `fire_and_forget_shell_commands_spawn_no_tagged_pane`
    (src/app/mod.rs:8085-8108) pins it: shell run leaves the pane count and
    the pre-existing focused pane's command_label untouched.

- [x] G3: full suite green after the change (lead-run).
  CHECK: cargo nextest run --no-fail-fast 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  23.335s] 3992 tests run: 3992 passed, 1 skipped
