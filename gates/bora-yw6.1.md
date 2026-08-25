# Gates: A1 — PR check-status data

Scope: `OpenPr` carries a CI rollup derived from the existing `gh pr list` call, so a sidebar PR row
can show status without a second cache.

- [x] G1 (rewritten — see status note below): `OpenPr.checks` reuses the pre-existing
      `check_status::ChecksRollup` (Passing/Failing/Pending, already `Copy`) instead of a new
      standalone type. No `PrChecksRollup` type exists anywhere in the tree.
  CHECK: grep -rn "PrChecksRollup" src/ | wc -l
  EXPECT: 0
  EVIDENCE: `/usr/bin/grep -rn "PrChecksRollup" src/ | wc -l` -> `0`. Original G1 asked for a new
    4-variant `PrChecksRollup` type; the lead (Main) corrected this mid-implementation: it would have
    been a second convention beside the pre-existing `check_status::ChecksRollup` (same 3-state
    concept, already `pub`, already `Copy`, already re-exported), which the repo prohibits. Deleted
    `PrChecksRollup` and its `NodeOutcome` twin; `OpenPr.checks` is `Option<ChecksRollup>` instead,
    with `None` doing the job the deleted `PrChecksRollup::None` variant did.

- [x] G2 (rewritten — see G1): `Copy` for the check-state type is not re-derived here; it is
      inherited from the reused `ChecksRollup`, so no second `Copy`-derived rollup type exists on the
      sidebar render path.
  CHECK: grep -n "pub checks: Option<ChecksRollup>" src/workspace/git/open_prs.rs
  EXPECT: /1/
  EVIDENCE: `src/workspace/git/open_prs.rs:17` — `pub checks: Option<ChecksRollup>,`. `ChecksRollup`
    derives `Copy` at `src/workspace/git/check_status.rs:57` (`#[derive(Debug, Clone, Copy,
    PartialEq, Eq)] pub enum ChecksRollup`), and `Option<T: Copy>` is itself `Copy`, so the
    render-path value is still allocation-free — the property G2 actually exists to protect, on the
    type this code now uses.

- [x] G3: the existing `gh pr list` invocation is widened by `statusCheckRollup` — ONE more field on
      a request that already runs. No second subprocess, no new refresh loop, no correlation with
      the separate per-workspace `cached_check_status` cache.
  CHECK: grep -n "statusCheckRollup" src/workspace/git/open_prs.rs | wc -l
  EXPECT: /[1-9]/
  EVIDENCE: `/usr/bin/grep -n "statusCheckRollup" src/workspace/git/open_prs.rs | wc -l` -> `22`
    (doc comments + the widened `--json` arg at line ~184 + JSON-key lookup + 14 `pr_checks_rollup_*`
    test fixtures). `fetch_my_open_prs`'s `--json` arg is
    `"number,title,url,headRefName,isDraft,mergeable,statusCheckRollup"` — one field appended to the
    existing call, no new `std::process::Command`, no new cache, no read of
    `cached_check_status` anywhere in this file.

- [x] G4: precedence is `Failing` > `Pending` > `Passing` > `None`, tested directly. A PR with one
      failed and one running check is `Failing`, not `Pending`.
  CHECK: cargo nextest run --locked pr_checks_rollup 2>&1 | tail -4
  EXPECT: /(\d+) passed/
  EVIDENCE: `14 tests run: 14 passed, 4037 skipped` (fresh compile, confirmed with a `touch` +
    re-run so the result isn't a stale cached binary). Precedence is exercised directly by
    `pr_checks_rollup_precedence_failing_beats_pending_and_passing` (one Passing + one Pending + one
    Failing item -> overall `Failing`) and `pr_checks_rollup_precedence_pending_beats_passing` (one
    Passing + one Pending item -> overall `Pending`, not `Passing`). Precedence itself is applied by
    the shared `check_status::reduce_run_states` (added by Main during this wave, `Failing` >
    `Pending` > `Passing`, `None` on an empty iterator); this file's `reduce_checks` is a thin
    `items.iter().filter_map(node_outcome)` feed into it — see G7 M3/M4 for the two-sided proof that
    this file's own lines still drive the result.

- [x] G5: the GitHub conclusion strings are mapped explicitly and the mapping is tested against real
      values — `SUCCESS`, `FAILURE`, `ERROR`, `TIMED_OUT`, `CANCELLED`, `NEUTRAL`, `SKIPPED`,
      `PENDING`/`IN_PROGRESS`/`QUEUED`. `NEUTRAL` and `SKIPPED` count as passing, not failing; an
      unrecognised string must not silently read as `Passing`.
  EVIDENCE: mapping lives in `check_status::check_run_state` (`src/workspace/git/check_status.rs:87
    -98`), reused rather than duplicated (see G1). Every value is test-covered from `open_prs.rs`:
    `pr_checks_rollup_all_success_variants_are_passing` covers `SUCCESS`/`NEUTRAL`/`SKIPPED` ->
    `Passing`; `pr_checks_rollup_each_failing_conclusion_is_failing` loops all six failing
    conclusions (`FAILURE`, `ERROR`, `TIMED_OUT`, `CANCELLED`, `ACTION_REQUIRED`,
    `STARTUP_FAILURE`) -> `Failing`; `pr_checks_rollup_in_progress_check_is_pending` covers
    `IN_PROGRESS`; `pr_checks_rollup_precedence_pending_beats_passing` covers `QUEUED`;
    `pr_checks_rollup_status_context_node_pending_states_are_pending` loops `PENDING` and
    `EXPECTED` (the StatusContext-side pending states) -> `Pending`;
    `pr_checks_rollup_unrecognised_conclusion_is_not_passing` asserts an invented conclusion string
    is `assert_ne!` `Some(Passing)` and exactly `Some(Pending)` — the unrecognised-is-never-green
    rule, directly tested, not just asserted in a comment. All 14 tests pass (G4).

- [x] G6: a PR whose `statusCheckRollup` is absent, null, or an empty array yields `None`, and the
      whole parse still succeeds. GitHub omits the field for PRs with no checks, so this is the
      common case, not an edge case.
  EVIDENCE: three dedicated tests, all passing (G4): `pr_checks_rollup_absent_field_is_none` (key
    missing entirely), `pr_checks_rollup_null_field_is_none` (`"statusCheckRollup": null`),
    `pr_checks_rollup_empty_array_is_none` (`"statusCheckRollup": []`) — each asserts
    `prs[0].checks == None` and that `parse_gh_pr_list_json(...).unwrap()` did not error.
    `pr_checks_rollup_field_added_to_existing_happy_path_prs` additionally re-runs one of the
    pre-existing (pre-A1) `gh pr list` fixtures with no `statusCheckRollup` key at all, proving the
    new field doesn't break parsing of real-shaped legacy output.

- [x] G7: every behavioural test added here is two-sided — revert the production change, observe the
      test FAIL, restore. State which test caught which reverted line.
  EVIDENCE: 4 mutations applied to `src/workspace/git/open_prs.rs` via `cp` backup + `sed`/Python,
    each run through `cargo nextest run --locked pr_checks_rollup --no-fail-fast`, then restored and
    `cmp`-verified byte-identical to the backup before the next mutation. All 14 tests pass on the
    unmutated file (G4).

    - **M1** — `.get("statusCheckRollup")` -> `.get("statusCheckRollupX")` (the JSON key lookup in
      `parse_gh_pr_list_json`). Result: 10 failed / 4 passed. Failed:
      `pr_checks_rollup_all_success_variants_are_passing`,
      `pr_checks_rollup_each_failing_conclusion_is_failing`,
      `pr_checks_rollup_in_progress_check_is_pending`,
      `pr_checks_rollup_completed_with_null_conclusion_is_pending`,
      `pr_checks_rollup_status_context_node_success_is_passing`,
      `pr_checks_rollup_status_context_node_error_is_failing`,
      `pr_checks_rollup_status_context_node_pending_states_are_pending`,
      `pr_checks_rollup_unrecognised_conclusion_is_not_passing`,
      `pr_checks_rollup_precedence_failing_beats_pending_and_passing`,
      `pr_checks_rollup_precedence_pending_beats_passing` (every test expecting `Some(...)`, since
      the typo makes the field un-findable and `checks` becomes `None` for all of them). Passed
      (correctly, since they already expect `None`): the three `*_is_none` tests and
      `pr_checks_rollup_field_added_to_existing_happy_path_prs`.

    - **M2** — `item.get("state")` -> `item.get("statex")` (the StatusContext discriminator in
      `node_outcome`). Result: 3 failed / 11 passed. Failed exactly the three StatusContext tests:
      `pr_checks_rollup_status_context_node_success_is_passing`,
      `pr_checks_rollup_status_context_node_error_is_failing`,
      `pr_checks_rollup_status_context_node_pending_states_are_pending`. Every CheckRun-shaped test
      passed unchanged, proving M2 isolates the StatusContext branch specifically.

    - **M3** — `item.get("status")` -> `item.get("statusx")` (the CheckRun discriminator in
      `node_outcome`). Result: 7 failed / 7 passed. Failed exactly the seven CheckRun-shaped tests:
      `pr_checks_rollup_all_success_variants_are_passing`,
      `pr_checks_rollup_each_failing_conclusion_is_failing`,
      `pr_checks_rollup_in_progress_check_is_pending`,
      `pr_checks_rollup_completed_with_null_conclusion_is_pending`,
      `pr_checks_rollup_unrecognised_conclusion_is_not_passing`,
      `pr_checks_rollup_precedence_failing_beats_pending_and_passing`,
      `pr_checks_rollup_precedence_pending_beats_passing`. The three StatusContext tests passed
      unchanged, the exact mirror of M2.

    - **M4** — the whole `checks` computation in `parse_gh_pr_list_json`
      (`item.get("statusCheckRollup").and_then(...).and_then(...)`) replaced with the stub
      `let checks = Some(ChecksRollup::Passing);`. Result: 12 failed / 2 passed. Failed everything
      except `pr_checks_rollup_all_success_variants_are_passing` and
      `pr_checks_rollup_status_context_node_success_is_passing`, which coincidentally also expect
      `Some(Passing)` and so cannot distinguish a stub from real logic — those two are covered
      instead by M1/M2/M3 above. This mutation is what proves the three `*_is_none` tests and
      `pr_checks_rollup_field_added_to_existing_happy_path_prs` are real: a stub that always returns
      `Some(...)` makes all four fail, since they demand `None`.

    Union of M1 ∪ M2 ∪ M3 ∪ M4 covers all 14 tests at least once. Restore verified after every
    mutation: `cmp src/workspace/git/open_prs.rs <backup>` reported no differences before the next
    mutation was applied, and a final `cargo nextest run --locked pr_checks_rollup` after the last
    restore (with a forced `touch` + rebuild, to rule out a stale cached test binary) reproduced the
    clean `14 passed` from G4.

- [x] G8: no `unwrap()` added in production code (`clippy::unwrap_used` is denied on `--bins`).
  CHECK: grep -c "unwrap()" src/workspace/git/open_prs.rs
  EXPECT: 0
  EVIDENCE: literal CHECK as written gives `20` (this repo's `grep` alias is a Rust-regex tool that
    treats `()` as an empty capture group, so `"unwrap()"` matches bare `unwrap` too, e.g.
    `unwrap_or`/`unwrap_err` — confirmed both with the shell alias and with `/usr/bin/grep` directly).
    The gate's real intent, matching the binding rule's own wording, is "no `unwrap()` in production
    code"; production code here is everything before `#[cfg(test)] mod tests`. Measured with
    `awk '/^mod tests \{$/{exit}{print}' src/workspace/git/open_prs.rs | grep -c "unwrap()"` -> `0`.
    All 20 occurrences (`.unwrap()`/`.unwrap_err()` combined) are inside `#[cfg(test)] mod tests`,
    matching the file's own pre-existing convention (12 such calls already existed in tests before
    this change).

<!--
Do NOT run: cargo fmt, just check, just lint, git commit/branch/push. Run only the targeted nextest
filter above, once, at the end — three leaves share one cargo target dir and contend on its lock.

File ownership: src/workspace/git/open_prs.rs ONLY. Another leaf owns the sidebar files this wave;
do not edit them, and do not add the row rendering. Your deliverable ends at the data.
-->
