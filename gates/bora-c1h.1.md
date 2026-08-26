# Gates — bora-c1h: sidebar Project view v3 (Ary-approved mock)

Bar: /Users/aryrabelo/Sites/temp-files/20260826-sidebar-mocks/sidebar-mocks.html (v3 final).
Every gate needs evidence. CHECK lines are runnable; EXPECT lines state the pass shape.

Ledger: 13 of 14 checked. G13 is the operator's step (live handoff cannot be run
from inside the session it would replace) — command given below.

## G1 — Group headers: underline, no ⬡
- [x] Project view group header rows render the group name with underline modifier and NO hexagon glyph; count stays right-aligned.
CHECK: cargo nextest run --bin bora -E 'test(/group_header|project_view/)' 2>&1 | tail -3
EXPECT: all matched tests pass; a capture test names the underline modifier on the group row and the absence of the ⬡ glyph.
EVIDENCE: `Summary [0.127s] 51 tests run: 51 passed, 3949 skipped`. Named test:
`ui::sidebar::capture::tests::v3_group_header_row_is_underlined_with_no_hexagon`
(`src/ui/sidebar/capture.rs:510`); unit-level sibling asserts `text.starts_with("▾ CNB")`,
`!text.contains('⬢')`, `text.ends_with("3/4")`.

## G2 — Workspace section line: chevron + uppercase bright name + dim branch
- [x] One section row per workspace: dim chevron (▾/▸ by collapse state), workspace name in bright + bold + UPPERCASE, branch dim after it. No second bright repo line.
CHECK: cargo nextest run --bin bora -E 'test(/workspace_section|section_row|project_view/)' 2>&1 | tail -3
EXPECT: pass; capture test shows uppercase name span (bright) and branch span (dim) on the same row as the chevron.
EVIDENCE: `Summary [0.160s] 56 tests run: 56 passed, 3944 skipped`. Named tests:
`section_row_line_shows_uppercase_name_and_dim_branch`,
`v3_section_row_shows_uppercase_name_dim_branch_and_worktree_marker`
(`src/ui/sidebar/capture.rs:527`).

## G3 — Branch "smaller" translates to dim-only (terminal truth)
- [x] Branch segment uses the dim style (no size change possible in grid); documented in code comment.
CHECK: grep -n "branch" src/ui/sidebar/project_view.rs | grep -ci "dim"
EXPECT: >= 1 code path styling the branch segment dim.
EVIDENCE: `1`. The branch span is `p.overlay1` in `section_row_line`
(`src/ui/sidebar.rs:1272`), which is the dim tone; the mock's `.br2` font-size drop has no
grid equivalent and the code comment says so.

## G4 — Worktrees are full workspace sections
- [x] Each worktree workspace renders as its own section (name · branch + its panes with ○), marked with ⌗; the condensed mauve "##name" row + ╰-children form is gone from Project view. ╰ survives ONLY for sibling panes within the same workspace.
CHECK: cargo nextest run --bin bora -E 'test(/worktree|project_view/)' 2>&1 | tail -3
EXPECT: pass; capture test shows a worktree section with ⌗ and its panes as ○ rows; no "##" prefix in rendered Project view text.
EVIDENCE: `Summary [2.815s] 228 tests run: 228 passed, 3772 skipped`. Named test:
`section_row_line_marks_worktree_checkouts_with_hilbert_glyph_main_gets_none` asserts
`worktree.contains('⌗')`, `!main.contains('⌗')`, `!worktree.contains("##")`.

## G5 — State cluster: git + PR icons right-aligned on section row
- [x] Section row carries right-aligned state: ahead ↑N / behind ↓N (or NF arrows), dirty glyph (change_set Unstaged non-empty), staged glyph (Staged non-empty), PR glyph + number (from Workspace.cached_check_status.pr) + checks ✓/pending/✗ via checks_rollup_glyph. Unknown checks NEVER classify green (repo rule). Cluster never truncates the workspace name to zero width; name ellipsizes first.
- [x] SCOPE NOTE (scout-verified): conflicts glyph DEFERRED — no merge-conflict detection exists anywhere in workspace/git/change_set.rs (porcelain UU/AA/DD bucket into Modified, change_set.rs:151-157). Code comment + bead note name this residual; a conflicts glyph requires a change_set.rs parsing change, out of scope here.
CHECK: cargo nextest run --bin bora -E 'test(/state_cluster|pr_chip|git_state|project_view/)' 2>&1 | tail -3
EXPECT: pass; tests cover ahead/behind/dirty/conflicts rendering, PR chip with checks rollup states, and unknown-conclusion -> pending (not green).
EVIDENCE: `Summary [0.105s] 52 tests run: 52 passed, 3948 skipped`.
Two mock-fidelity defects found by reading the mock CSS rather than the mock picture, both
fixed in this pass:
1. **Cluster was inline, mock floats it right** (`.sec .st { float: right }`,
   `sidebar-mocks.html:63`). `section_row_line` now pads the slack between the branch and
   the cluster so the cluster's right edge is the row's right edge, and skips the padding
   when there is no cluster so plain rows stay short. New test
   `section_row_line_pins_the_state_cluster_to_the_right_edge`; mutation-verified — deleting
   the padding block reddens exactly that test (`1 test run: 0 passed, 1 failed`).
2. **Behind arrow was red, mock says yellow** (`.behind { color: var(--yellow) }`,
   `sidebar-mocks.html:67`; red is `.fail` only). Fixed with the reason in a code comment:
   red spent on "behind origin" makes a real CI failure harder to spot. Test renamed to
   `section_row_line_ahead_green_behind_and_dirty_and_staged_yellow` and now asserts both
   `fg == yellow` and `fg != red`.

## G6 — NF glyph set behind config, plain-unicode default
- [x] New config key selects Nerd Font glyphs vs plain unicode fallback. Plain unicode is the default so unpatched terminals never see tofu; docs/next config reference lists the key (maintenance test enforces).
CHECK: python3 -m unittest scripts.test_config_reference_check 2>&1 | tail -2
EXPECT: OK — the key exists in src/config AND docs/next/website/src/data/config-reference.json.
EVIDENCE: `Ran 15 tests in 0.014s` / `OK`.

## G7 — row_gap=1 between workspaces, never inside a workspace block
- [x] One blank row after each workspace block in Project view (incl. worktree blocks); no blank row between a pane and its ╰ siblings; no blank row after the last workspace before a group header (or: consistent rule, documented).
CHECK: cargo nextest run --bin bora -E 'test(/row_gap|project_view/)' 2>&1 | tail -3
EXPECT: pass; the three-pass lockstep test (entry_row_height agreement) still compiles/passes with gap rows included.
EVIDENCE: `Summary [0.100s] 52 tests run: 52 passed, 3948 skipped`. Named test:
`v3_row_gap_produces_blank_rows_between_workspace_blocks` (`src/ui/sidebar/capture.rs:572`).

## G8 — Three-pass lockstep intact
- [x] workspace_list_visible_count / compute_workspace_list_areas / render_workspace_list all derive every row height via entry_row_height; new row shapes flow through all three passes.
CHECK: cargo nextest run --bin bora -E 'test(/lockstep/)' 2>&1 | tail -3
EXPECT: pass; a new workspace-section + gap variant is handled in the non-wildcard matches (compile-enforced).
EVIDENCE: `Summary [0.013s] 4 tests run: 4 passed, 3996 skipped`.

## G9 — Mouse: section row = collapse toggle, pane row = focus; right-click targets intact
- [x] Clicking the section row toggles collapse; clicking a ○ pane row focuses it; right-click on section rows still opens the Project view context menus (bora-uqv routing keeps working).
CHECK: cargo nextest run --bin bora -E 'test(/sidebar.*(click|mouse|context)|project_row/)' 2>&1 | tail -3
EXPECT: pass; existing bora-uqv menu tests stay green against the new row targets.
EVIDENCE: `Summary [0.096s] 29 tests run: 29 passed, 3971 skipped`.

## G10 — Goldens: verify isolation first, re-attribute only if shifted
- [x] desktop_full_app_semantic_frame_is_characterized + mobile sibling are REPO-view goldens (default ViewMode::Repo, app/state.rs:2832); project_view.rs module doc declares Flat/Repo untouched. Run them UNCHANGED first: if byte-identical, no bump (record that as evidence). If they shift, a shared function leaked scope into the Repo path — isolate it, or re-attribute via the probe technique with before/after rows in the comment.
CHECK: cargo nextest run --bin bora -E 'test(/semantic_frame_is_characterized/)' 2>&1 | tail -3
EXPECT: pass — either unchanged (isolation proven) or re-attributed with the probe comment.
EVIDENCE: both pass UNCHANGED — no hash bump, no probe needed:
`PASS mobile_full_app_semantic_frame_is_characterized`,
`PASS desktop_full_app_semantic_frame_is_characterized`,
`2 tests run: 2 passed`. Isolation from the Repo path is therefore proven by the goldens
themselves rather than asserted in prose.

## G11 — Full gate green
- [x] just check passes on the integrated tree.
CHECK: just check 2>&1 | tail -3
EXPECT: OK / no error lines.
EVIDENCE: green after one `cargo fmt` (the fmt check caught a hand-wrapped `assert!` in the
G1 test). Full run: `3999 tests run: 3999 passed, 1 skipped` (bora unit),
`Ran 143 tests in 7.591s / OK (skipped=1)` (maintenance scripts), plugin marketplace
`31 pass / 0 fail`, `5 tests` installer suite.

## G12 — Changelog + version
- [x] docs/next/CHANGELOG.md Added entry (user-facing); Cargo.toml bumped to 0.45.7 in the same commit.
CHECK: grep -n "0.45.7" Cargo.toml docs/next/CHANGELOG.md | head -3
EXPECT: both files name 0.45.7.
EVIDENCE: `Cargo.toml:3:version = "0.45.7"`. The changelog entry is in `## Unreleased`
and deliberately carries NO version number — the EXPECT line was wrong about this: per the
repo's docs rule, `just release-prepare` is the only thing that stamps a version onto a
changelog section, and pre-stamping it is what `scripts/changelog.py check-history-sync`
exists to catch. Entry text verified present under `## Unreleased` / `### Added`.

## G13 — Live smoke
- [ ] bora 0.45.7 installed + live-handoff; real sidebar in Project view shows the v3 form on Ary's actual data; bora-uqv right-click menus still open.
EVIDENCE: install half done and verified — `just install` printed
`installed: bora 0.45.7 (v0.8.2[2c042bb2].bora-45.7)` and `bora --version` agrees.
`bora status` shows `client 0.45.7` / `server 0.45.6` / `restart_needed: yes`, so the
running server still holds the old inode and the new sidebar is not on screen yet.
HANDOVER (operator step, not abandoned): `bora server live-handoff`. Not run from here on
purpose — this agent runs inside a pane of the very server the handoff replaces, so running
it from here risks dropping the session mid-turn, and a failed handoff would take Ary's live
agents with it.

## G14 — Critic round
- [x] Blind A/B: implementation capture vs v3 mock bar, fresh harsh critic, two-line verdict recorded. If ours loses, loop the gap until it wins or a named residual is documented.
EVIDENCE: `/Users/aryrabelo/Sites/temp-files/20260826-sidebar-mocks/gauntlet/round1/verdict.txt`
→ `{"ours_won":true,"gap":"its section-row state cluster carries no PR chip or checks-state
colors at all, so the merged-purple / green-pass / red-fail / never-green-on-unknown
contract is absent"}`. The named gap belongs to the theirs-side extract, not to ours.
Two localized flags the critic raised against ours were adjudicated rather than waved off:
the right-edge float and the behind-arrow colour were REAL and are fixed under G5; the group
header's chevron is an intentional deviation (group collapse needs the affordance and the
static mock could not model it).

## Residuals (named, not silently dropped)
- Conflicts glyph: needs merge-conflict parsing in `workspace/git/change_set.rs` (G5 scope note).
- Group-header chevron: intentional deviation from the mock, documented above.
