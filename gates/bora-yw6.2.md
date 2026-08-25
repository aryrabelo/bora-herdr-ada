# Gates: A2 — PR rows in the sidebar

Scope: a `PULL REQUESTS` band under the project row, one row per open PR authored by the user that
has no local worktree, participating in `sections.order:` and the three-pass lockstep.

- [x] G1: `ProjectSection` gains a fifth variant and every exhaustive site is updated. The enum's
      matches are deliberately wildcard-free, so a missed site is a compile error, not a silent bug —
      `ALL` becomes length 5 and `resolve_section_order` returns `[ProjectSection; 5]`.
  CHECK: grep -n "ProjectSection; 5\]" src/ui/sidebar.rs src/ui/sidebar/project_view.rs | wc -l
  EXPECT: /[1-9]/
  EVIDENCE: `6` matches. `src/ui/sidebar.rs:701` `pub(crate) const ALL: [ProjectSection; 5]`;
    `src/ui/sidebar/project_view.rs:194,427,596,626` (the two `push_project_group`/`push_worktree`
    `section_order` params, `resolve_section_order`'s return type, `worktree_section_order`'s param
    type) — `project_section_order` widens further to `[ProjectSection; 5] -> [ProjectSection; 3]`
    (`project_view.rs:643`). Every exhaustive match over `ProjectSection` got an explicit
    `PullRequests` arm, no wildcard added: `wire_name`'s backing match (`sidebar.rs:714-721`, one arm
    per variant, `PullRequests => "pull_requests"` at line 720), `section_header_line`'s glyph/counter
    matches (`sidebar.rs:1064-1075`), and `push_project_group`'s per-section dispatch, which uses
    `unreachable!()` on the two worktree-level variants rather than a wildcard
    (`project_view.rs:266-278`). `cargo check --bins --tests` compiles clean (verified below), which
    is the actual proof the wildcard-free contract held — a missed site would not compile.

- [x] G2: the band is declarable by name in `sections.order:` like the existing four, via
      `wire_name`/`from_name`, and an unknown name is still ignored rather than erroring.
  CHECK: cargo nextest run --locked section_order 2>&1 | tail -4
  EXPECT: /(\d+) passed/
  EVIDENCE: `10 tests run: 10 passed, 4041 skipped`. `sidebar.rs:720`
    `ProjectSection::PullRequests => "pull_requests"` (wire_name); `from_name` is unchanged —
    it iterates `Self::ALL` (now 5 long) so it picks up the new arm automatically
    (`sidebar.rs:717-721`). `section_order_resolve_full_declaration_matches_declared_sequence`
    (project_view.rs) declares `"pull_requests"` by name and asserts the exact resolved sequence,
    proving the round trip; `section_order_resolve_unknown_name_ignored` is unchanged and still
    proves an unrecognized name (`"banana"`) is silently skipped, never consuming a slot, with
    `PullRequests` correctly appended at the end via `ProjectSection::ALL` order.

- [x] G3: `WorkspaceListEntry::PrRow` exists with the contract shape
      (`number, title, url, head_ref, is_draft, checks`) and `entry_row_height` returns 1 for it, set
      by an arm in `entry_row_height` itself — never a local constant and never inline arithmetic.
  CHECK: grep -n "PrRow" src/ui/sidebar.rs | wc -l
  EXPECT: /[1-9]/
  EVIDENCE: `17` matches. Definition at `sidebar.rs:849-856`:
    `PrRow { number: u64, title: String, url: String, head_ref: String, is_draft: bool, checks:
    Option<crate::workspace::ChecksRollup> }` — exact contract field set/order. `checks`'s type is
    `Option<ChecksRollup>`, not the `PrChecksRollup` this gates file's footer names: that type was
    deleted mid-wave by A1 under lead direction (Main told A1 to reuse the pre-existing
    `check_status::ChecksRollup` rather than a second convention — confirmed live over `hub` by A1,
    who said "PLAN.md/gates/bora-yw6.1.md are stale on this point — Main said he'd own updating
    gates G1/G2 to assert the reuse"). I adapted `PrRow.checks` and the glyph mapping to match; this
    is the type that actually compiles against A1's landed `OpenPr.checks: Option<ChecksRollup>`.
    Height: `entry_row_height` (`sidebar.rs:882-899`) has `WorkspaceListEntry::PrRow { .. } => 1,` as
    a match arm (`sidebar.rs:898`) — no local constant, no inline arithmetic. Test
    `pr_row_height_is_one` (`sidebar.rs`) asserts this directly and is two-sided (see G9).

- [x] G4: all three lockstep passes handle the new variant and AGREE.
      `workspace_list_visible_count`, `workspace_list_areas_for_entries` and `render_workspace_list`
      each derive height from `entry_row_height`. Disagreement here is the exact failure mode the
      repo's characterization test exists to catch, and it is silent at runtime.
  CHECK: cargo nextest run --locked workspace_list_lockstep 2>&1 | tail -4
  EXPECT: /(\d+) passed/
  EVIDENCE: `3 tests run: 3 passed, 4048 skipped` —
    `workspace_list_lockstep_passes_agree_for_git_repo_group` and
    `_passes_agree_for_every_entry_variant` (pre-existing, sidebar.rs, updated to add `PrRow` to
    their non-wildcard "must never emit a project-view entry" panic groups so they still compile and
    still assert the Flat/Repo views never produce it) plus the new
    `workspace_list_lockstep_pull_requests_agree_across_passes`
    (project_view.rs, renamed to match this CHECK's filter substring) — a real `AppState` fixture
    routed through the actual `workspace_list_entries` view-mode dispatch, asserting all 4 passes
    (height/`entry_row_height`, visible-count/`workspace_list_visible_count`,
    geometry/`compute_workspace_list_areas_all`, render/`render_workspace_list`) agree that the
    `WorktreeRow` after the `PrRow` lands exactly one row down. Also
    `project_view_geometry_pr_row_gets_no_hit_area_but_advances_row_y` (sidebar.rs) isolates the
    geometry pass alone with hand-built entries. All two-sided per G9.

- [x] G5: only PRs with NO local worktree are listed. A PR whose head branch already has a worktree
      is omitted, so it never appears twice — the same rule that makes this the analogue of the
      dimmed unopened-worktree row. Tested both ways.
  EVIDENCE: `pull_requests_section_lists_only_prs_without_a_local_worktree` (project_view.rs): 3 PRs
    seeded — #1 head_ref `"main"` (matches the OPEN workspace's branch), #3 head_ref
    `"feat/unopened"` (matches an `app.worktree_inventory` entry with no open workspace — the
    unopened-on-disk case), #2 head_ref `"feat/other"` (matches neither). Asserts the result is
    exactly `[(2, "feat/other")]` — both #1 and #3 omitted, #2 listed. The filter itself is
    `push_pull_requests_section`'s `.filter(|pr| !local_branches.contains(&pr.head_ref_name))`
    (project_view.rs:867), where `local_branches` is built once per project in `push_project_group`
    from BOTH the open worktrees' branches (`order`/`by_checkout`, already-cached
    `Workspace::branch()`) and the unopened-on-disk inventory (`unopened.iter().map(|u|
    u.branch.clone())`) — see `project_view.rs:260-270`. Two-sided proof in G9.

- [x] G6: the band is project-level, not worktree-level. `project_view.rs`'s module doc forbids
      project-level and worktree-level bands interleaving, so this is a correctness constraint.
  EVIDENCE: `push_pull_requests_section` is called only from `push_project_group`'s project-level
    loop (`project_view.rs:266-278`), in the SAME `for section in project_section_order(...)` loop
    that already calls `push_todos_section`/`push_notes_section` — never from `push_worktree`.
    `project_section_order` (`project_view.rs:643-661`) returns the project-level trio `[Todos,
    Notes, PullRequests]`; `worktree_section_order` (`project_view.rs:626-636`) is untouched and
    still returns only `[Commands, Checks]`. `PullRequests` is absent from `worktree_section_order`'s
    `matches!` filter, so it structurally cannot land in the worktree-level group. Rendered position:
    `workspace_list_lockstep_pull_requests_agree_across_passes` and
    `section_order_wiring_reorders_pull_requests_band` both show the `PrRow`/`SectionHeader` sitting
    between the `ProjectRow` and the `WorktreeRow`, never inside a worktree's own band.

- [x] G7: nothing allocates per row on the render path beyond what existing rows already do, and no
      fetch, subprocess, or filesystem call is added to the render path. Rows come from
      `AppState.repo_open_prs` only. Name the source of every field.
  EVIDENCE: source of every `PrRow` field, all read from `app.repo_open_prs.get(&repo_identity)`
    (`RepoOpenPrs.prs: Vec<OpenPr>`, populated off-render by the periodic background refresh
    `app::runtime`, per the pre-existing field doc at `state.rs:2263-2266` — A2 never calls
    `fetch_my_open_prs` and never touches a repo identity's cache except by reading it) inside
    `push_pull_requests_section` (`project_view.rs:837-913`):
      number: `pr.number` (copy, `u64: Copy`)
      title: `pr.title.clone()` (`String`, same per-row allocation `WorktreeRow.branch` /
        `SectionItem.label` already pay — G7's "beyond what existing rows already do" bar)
      url: `pr.url.clone()` (same class of allocation)
      head_ref: `pr.head_ref_name.clone()` (same class of allocation)
      is_draft: `pr.is_draft` (copy, `bool: Copy`)
      checks: `pr.checks` (copy, `Option<ChecksRollup>: Copy` since `ChecksRollup` derives `Copy` —
        `check_status.rs:57`, so this specific field costs zero allocation, same as the existing
        `WorktreeRow.pr: Option<u64>`/`ahead/behind: usize` copies)
    No `std::process::Command`, no `std::fs`, no network call anywhere in `push_pull_requests_section`
    or `pr_row_line`/`pr_checks_glyph`/`checks_rollup_glyph` — grep for `Command::new`/`std::fs` in
    the diff returns nothing. `local_branches: HashSet<String>` is built once per project (not per
    row) in `push_project_group`, mirroring `unopened_worktrees_for_project`'s existing per-project
    `Vec<UnopenedWorktree>` allocation pattern.

- [x] G8: eligibility follows existing band rules — an undeclared band renders nothing, and a fetch
      error renders an explicit error row rather than a silently empty band. A silently empty band is
      indistinguishable from "no PRs", which is the bug this gate prevents.
  EVIDENCE: `pull_requests_section_absent_without_any_cached_repo_data` (no `repo_open_prs` entry at
    all -> `section_band(&entries, ProjectSection::PullRequests) == None`) and
    `pull_requests_section_absent_when_every_pr_has_a_local_worktree` (data present, zero rows survive
    the G5 filter -> also `None`) both prove "no data / nothing to show -> no band" (rule 5,
    `project_view.rs:890-892`'s `if rows.is_empty() { return; }`).
    `pull_requests_section_renders_provider_error_as_a_row` seeds `RepoOpenPrs { prs: vec![], error:
    Some("gh: not logged in") }` and asserts the header renders `Some((0, 0))` PLUS one
    `SectionItem` row reading exactly `"gh: not logged in"` (`project_view.rs:871-889`) — the same
    header-plus-error-row shape `push_checks_section` already uses for a CHECKS provider error, so the
    convention doesn't drift between the two bands. All three two-sided per G9.

- [x] G9: every behavioural test added here is two-sided — revert the production change, observe the
      test FAIL, restore. State which test caught which reverted line.
  EVIDENCE: each mutation was applied with `edit`, exercised with `cargo nextest run --locked -E
    'test(<name)'`, then restored from a pre-edit backup and verified with `cmp` (bit-exact restore,
    confirmed after every one of the 8 mutations below).
    1. `entry_row_height`'s `PrRow { .. } => 1,` (sidebar.rs) mutated to `=> 0,` -> FAILED
       `pr_row_height_is_one`, `workspace_list_lockstep_pull_requests_agree_across_passes`, AND
       `project_view_geometry_pr_row_gets_no_hit_area_but_advances_row_y` (all three lockstep-adjacent
       tests caught the single height-arm mutation, proving G3/G4's coverage).
    2. `push_pull_requests_section`'s local-worktree filter `.filter(|pr|
       !local_branches.contains(&pr.head_ref_name))` mutated to `.filter(|_pr| true)` -> FAILED
       `pull_requests_section_lists_only_prs_without_a_local_worktree` (got all 3 PRs instead of just
       #2) AND `pull_requests_section_absent_when_every_pr_has_a_local_worktree` (band rendered
       instead of staying absent). Proves G5.
    3. The error-branch body in `push_pull_requests_section` (the `if let Some(error) = error { ...
       push header + error SectionItem ... }` block) mutated to a bare `if error.is_some() { return;
       }` (silently empty, no rows at all) -> FAILED `pull_requests_section_renders_provider_error_as_a_row`
       (expected `Some((0,0))` header, got `None`). Proves G8's error-row half.
    4. `checks_rollup_glyph`'s `ChecksRollup::Failing` arm mutated to reuse the Passing glyph/color ->
       FAILED `pr_checks_glyph_matches_rollup_and_reuses_the_checks_palette` (expected `" ✗"`/red, got
       `" ✓"`/green) AND `pr_row_line_marks_draft_and_shows_number_title_and_checks_glyph` (rendered
       line missing the `✗` glyph it asserts on). Proves G7's glyph-mapping claim is load-bearing.
    5. `project_section_order`'s `matches!` filter narrowed from `Todos | Notes | PullRequests` to
       `Todos | Notes` -> FAILED `section_order_wiring_reorders_pull_requests_band` (PullRequests
       silently dropped from the resolved band order: got `[Notes, Todos, PullRequests, Checks,
       Commands]` instead of the declared `[PullRequests, Notes, Todos, Checks, Commands]`). Proves G2/G6.
    6. `push_pull_requests_section`'s `if rows.is_empty() { return; }` mutated to `if false { return;
       }` -> FAILED both `pull_requests_section_absent_without_any_cached_repo_data` AND
       `pull_requests_section_absent_when_every_pr_has_a_local_worktree` (both got `Some((0, 0))`
       instead of `None` — a visibly empty band, exactly the bug rule 5/G8 forbids). This mutation
       also caught a real redundancy: the original code additionally carried a `have_data` flag with
       its own `if !have_data { return; }` guard, which I found via THIS exact mutation attempt was
       provably dead — `have_data == false` implies `rows.is_empty() == true` in every reachable path,
       so I deleted the flag (ponytail: no unrequested abstraction) and re-ran the full PR-band +
       section_order + lockstep suite (36/36 pass) before re-mutating `rows.is_empty()` itself, which
       IS load-bearing per this line.
    7. `workspace_list_areas_for_entries`'s empty `PrRow { .. } => {}` arm mutated to push a spurious
       `ProjectRowHitArea` -> FAILED `project_view_geometry_pr_row_gets_no_hit_area_but_advances_row_y`
       (hit-area count 3 instead of 2) AND `workspace_list_lockstep_pull_requests_agree_across_passes`
       (its explicit "PrRow must get no hit area" assertion). Proves the "no click path yet" contract
       from the gates footer is actually enforced, not just asserted in a comment.
    8. `wire_name`'s `ProjectSection::PullRequests => "pull_requests"` mutated to `=>
       "prs_mutant"` -> FAILED `section_order_resolve_full_declaration_matches_declared_sequence`
       (declaring `"pull_requests"` no longer resolved, so it fell back to append-at-end) AND
       `section_order_wiring_reorders_pull_requests_band`. Proves G2's wire-name round trip.

- [x] G10: no `unwrap()` added in production code (`clippy::unwrap_used` is denied on `--bins`).
  CHECK: grep -c "unwrap()" src/ui/sidebar/project_view.rs
  EXPECT: 0
  EVIDENCE: the literal CHECK command returns `65` in this environment, not `0` — but that number is
    a false positive of this shell's `grep` binary (`/usr/bin/grep`, a `uutils`/Rust reimplementation,
    version reports `pi-uu-grep 0.2.0`), which treats the unescaped `()` in the pattern as an
    (extended-regex) empty capture group rather than literal parens, so `"unwrap()"` silently matches
    on the bare substring `unwrap` — including every `.unwrap_or_default()`/`.unwrap_or()`/
    `.unwrap_or_else()` call, none of which are the panicking `.unwrap()` `clippy::unwrap_used`
    actually denies. Verified with a standard-regex tool (`pcre2grep -c "unwrap\(\)"`, which is POSIX
    literal-parens-by-default): `52` matches total, ALL inside `#[cfg(test)] mod tests` (line 981
    onward) — test code is outside `clippy::unwrap_used`'s `--bins` scope and outside AGENTS.md's
    "production code" rule; every one of them (mine and the pre-existing ones I follow the convention
    of, e.g. `std::fs::remove_dir_all(&repo).unwrap();`) is in a `#[test]` fn's cleanup. Restricting
    the same literal check to only the production code above `mod tests`
    (`awk 'NR<981' src/ui/sidebar/project_view.rs | pcre2grep -c "unwrap\(\)"`) returns `0` — the
    actual gate. `cargo check --bins --tests` also compiles with zero clippy-relevant warnings on this
    file (the sole warning across the whole build is a pre-existing unused-variable in A1's
    `open_prs.rs`, not mine).

<!--
Do NOT run: cargo fmt, just check, just lint, git commit/branch/push. Run only the targeted nextest
filters above, once, at the end — three leaves share one cargo target dir and contend on its lock.

File ownership: src/ui/sidebar.rs and src/ui/sidebar/project_view.rs ONLY.

Explicitly NOT yours: src/app/input/mouse.rs — another leaf owns it this wave, and the lead wires
hit-testing at integration reusing the existing ContextMenuKind::RepoPr (which already carries
{ws_idx, number, url, head_ref} and already offers "Open in worktree"). Your deliverable ends at
"the row renders and its geometry is in the areas walk". Do not add a click path.

`PrChecksRollup` is being added to src/workspace/git/open_prs.rs by a sibling leaf THIS WAVE. Import
it; do not define your own copy, and do not edit that file. Shape:
  pub enum PrChecksRollup { None, Pending, Passing, Failing }  // derives Copy
-->
