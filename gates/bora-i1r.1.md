# Gates: bora-i1r.1 — check provider contract + gh as built-in provider

Scope: a checks provider is a command template answering {repo, dir?, branch}
with JSON `[{name,status,conclusion}]` == `CheckRun`
(`src/workspace/git/check_status.rs`); today's `fetch_check_status` (gh pr
view) becomes the built-in `gh` provider behind that contract; not-applicable
is distinguished from error. Owned paths: `src/workspace/git/check_status.rs`
(plus a sibling module file if the refactor splits one out), this file. NOT
owned: sidebar render, config sections wiring (bora-i1r.2).

- [x] G1: gh provider output is identical to today's path — a characterization
  test feeds recorded `gh pr view` JSON through the provider and asserts the
  same `CheckRun`/`WorkspaceCheckStatus` the current parser produces (mixed
  SUCCESS/FAILURE/pending/legacy StatusContext shapes).
  CHECK: cargo nextest run -E 'test(/characteriz|gh_provider|check_status/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.035s] 28 tests run: 28 passed, 3927 skipped
  (src/workspace/git/check_provider.rs:278) feeds recorded gh JSON (CheckRun
  SUCCESS + FAILURE + IN_PROGRESS, StatusContext SUCCESS + PENDING) through
  `CheckProvider::gh().run_with` and asserts equality with
  `parse_gh_pr_json` plus exact pinned `CheckRun` values;
  `gh_provider_characterization_uses_legacy_argv` (:337) pins program `gh`
  and the legacy argv `[pr, view, <branch>, --json,
  number,title,state,url,statusCheckRollup,mergeable]`. PASS by inspection.

- [x] G2: a fake script provider (a test fixture executable/script emitting the
  JSON contract) is executed through the provider machinery and its rows parse
  into `CheckRun`s — proving the contract, not gh, is the integration point.
  CHECK: cargo nextest run -E 'test(/fake|script|provider/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   9.903s] 47 tests run: 47 passed, 3908 skipped
  (src/workspace/git/check_provider.rs:378) builds a stub provider ("fake-ci",
  argv `checks {branch} --dir {dir}`) whose parser is the generic contract
  parser `parse_contract_json` (:208) and runs it through the provider
  machinery (`run_with`, :126) with the injected exec seam — local convention
  forbids spawning subprocesses in tests (verified: sibling modules
  open_prs/issues/check_status test parse fns only), so the seam is a
  fabricated `CommandResult`. Rows parse to exact `CheckRun`s.
  `fake_script_provider_substitutes_branch_and_dir` (:416) pins `{branch}`/`{dir}`
  template substitution; `script_provider_contract_json_requires_an_array`
  (:441) pins contract shape. PASS by inspection.

- [x] G3: provider failure renders as error (`WorkspaceCheckStatus.error`
  populated), never silently empty; not-applicable (no provider configured /
  no PR) yields no rows and no error. Tests pin both.
  CHECK: cargo nextest run -E 'test(/error|not_applicable|unavailable/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   1.593s] 89 tests run: 89 passed, 3866 skipped
  (src/workspace/git/check_provider.rs:21). Not-applicable pinned by
  `gh_provider_no_pr_is_not_applicable` (:448) and
  `script_provider_not_applicable_marker_is_not_an_error` (:459) — unit
  variant, no rows, no error. Error pinned by
  `gh_provider_failure_is_error_never_silently_empty` (:467),
  `gh_provider_spawn_failure_is_error` (:495),
  `gh_provider_unparseable_output_is_error` (:506), and shaping preserved by
  `gh_provider_error_message_shaping_is_preserved` (:478). Legacy mapping to
  `WorkspaceCheckStatus.error` pinned by
  `legacy_status_maps_error_to_error_field_never_empty`
  (src/workspace/git/check_status.rs:425) and
  `legacy_status_maps_not_applicable_to_historical_no_pr_error` (:415).
  PASS by inspection.

- [x] G4: no behavior regression in the existing check-status tests.
  CHECK: cargo nextest run -E 'test(/check/)' 2>&1 | tail -5
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   1.103s] 85 tests run: 85 passed, 3870 skipped
  (rollup + parse_gh_pr_json suites, lines 217-397) are untouched; the parser
  `parse_gh_pr_json` (:83) and `checks_rollup` (:40) bodies unchanged. The
  gh provider's parse path delegates to the same `parse_gh_pr_json`
  (check_provider.rs:196). PASS by inspection.

- [x] G5: full suite green after the change (lead-run).
  CHECK: cargo nextest run 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  22.860s] 3954 tests run: 3954 passed, 1 skipped
