# Gates: sidebar attention (toast confirm + quiet promotion + waiting counter)

Scope: (A) confirm the finish/blocked toast is already enabled on this machine;
(C) a pane in a background workspace that goes quiet for `ui.idle_attention_seconds`
(default 300, 0=off) is promoted to the existing unseen-attention channel; (B) the
sidebar's top margin row shows an aggregate "N waiting" counter (red when any
unseen pane is blocked). Flat/Repo/Project/Folders rows are otherwise untouched.

- [x] G1: A confirmed — dotfiles config (the real source; `~/.config/bora/config.toml` is a nix-store symlink) already ships `[ui.toast] delivery = "terminal"`.
  CHECK: grep -A1 '^\[ui.toast\]' ~/Sites/dotfiles-2026/dotfiles/bora/config.toml
  EXPECT: /delivery = "terminal"/
  EVIDENCE: [ui.toast] | delivery = "terminal"

- [x] G2: Output stamp — a PTY read chunk records `last_output_at` on the pane runtime; None until first output (so restored shells never mass-promote).
  CHECK: cargo nextest run -E 'test(process_pty_bytes_sets_last_output)' 2>&1 | tail -4
  EXPECT: /1 test run: 1 passed/
  EVIDENCE: ──────────── | Summary [   0.015s] 1 test run: 1 passed, 4231 skipped

- [x] G3: Quiet promotion — `App::promote_quiet_panes` flips `seen=false` for quiet panes in background workspaces; skips the active workspace, channel workspaces, already-unseen panes, and runs forever-off when `ui.idle_attention_seconds = 0`.
  CHECK: cargo nextest run -E 'test(promote_quiet_panes)' 2>&1 | tail -4
  EXPECT: /passed/
  EVIDENCE: ──────────── | Summary [   0.021s] 2 tests run: 2 passed, 4230 skipped

- [x] G4: Tick wiring — the helper is called from BOTH scheduled-tick paths (App tick and `handle_scheduled_tasks_headless`), the projects.yml-poll rule.
  CHECK: grep -rn "promote_quiet_panes" src/app/runtime.rs src/server/headless.rs
  EXPECT: /runtime\.rs.*promote_quiet_panes/
  EVIDENCE: src/server/headless.rs:4884:        // helper (`App::promote_quiet_panes`), same drift rule as the two | src/server/headless.rs:4886:        changed |= self.app.promote_quiet_panes(now);

- [x] G5: Waiting counter — the sidebar margin row renders "N waiting" when the count is > 0 (red when any unseen pane is blocked, yellow otherwise) and renders nothing at 0.
  CHECK: cargo nextest run -E 'test(waiting_counter)' 2>&1 | tail -4
  EXPECT: /passed/
  EVIDENCE: ──────────── | Summary [   0.011s] 1 test run: 1 passed, 4231 skipped

- [x] G6: Config — `ui.idle_attention_seconds` parses (default 300; 0 allowed as off).
  CHECK: cargo nextest run -E 'test(idle_attention_seconds)' 2>&1 | tail -4
  EXPECT: /passed/
  EVIDENCE: ──────────── | Summary [   0.011s] 1 test run: 1 passed, 4231 skipped

- [x] G7: Mutation check — breaking the promotion gate and the counter gate redden G3/G5 respectively.
  CHECK: manual mutation run, revert via inverse sed
  EXPECT: /tests fail under mutation, pass after revert/
  EVIDENCE: mutation A (counter gate → `if false`): waiting_counter FAILed 0 passed; mutation B (active-workspace skip removed): flips test FAILed 0 passed. Both restored via inverse sed (guard verified with grep -F); promote tests 2/2 green after revert.

- [x] G8: Repo gate — `just check` passes.
  CHECK: just check 2>&1 | tail -5
  EXPECT: /OK/
  EVIDENCE: OK | docs reminder: if this changes user-facing behavior, make sure the relevant release docs are updated or called out before release.

- [x] G9: Closeout — version bump, `docs/next/CHANGELOG.md` entries, commit landed.
  CHECK: git log --oneline -1
  EXPECT: /attention|waiting|quiet/
  EVIDENCE: 61ed837c feat(sidebar): waiting counter and quiet-pane promotion for background terminals (v0.45.36)
