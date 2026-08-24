# Gates: round 2 — A (land C) + B (real DB backup) + C (bora-5ia, bora-6ps)

## A — land the sidebar work

- [x] GA1: the sidebar commit is on `origin/main`, not just local.
  CHECK: bash -c "git log origin/main --oneline -1 | cut -c1-60"
  EXPECT: /unopened worktree rows/
  EVIDENCE: 6998eab5 feat(sidebar): unopened worktree rows and honored w

## B — a real off-machine database backup

- [x] GB1: the remotesapi route is MEASURED, not assumed. A Dolt sql-server started with `--remotesapi-port` can be cloned by a client, which would put a full database on this machine and let the push run from here, where GitHub credentials exist. Probe it before concluding the route is closed.
  EVIDENCE: Closed, measured two ways. Port probe against dolt.bugtoprompt.com: 50051, 50052, 8080, 3308, 4321 all refused. And the server does not know the variable at all — `SELECT @@dolt_remotesapi_port` returns `Error 1105 (HY000): Unknown system variable`, so it was not started with the flag (Dolt v2.2.3). A client-side `dolt clone` of the live database is therefore impossible. `mysqldump` over the MySQL wire protocol was the remaining client-side idea: no `mysql`/`mysqldump` binary on this machine, and building a 28-table dump pipeline is tooling nobody asked for and nothing would consume.

- [x] GB2: either a full-database off-machine backup exists and round-trips, or the exact blocking credential is registered with `ops request` so the owner can fill it, and the ABANDON line names which.
  EVIDENCE: Registered, not fixed. `ops request dolthub-remote-token --ref "op://Development/DoltHub bora/credential" --project bora`, carrying the full `howto` (create the DoltHub db, take the credential, then `bd backup init https://doltremoteapi.dolthub.com/<user>/bora-beads && bd backup sync`) and the reason (`bd dolt push` refused, no remotesapi). `ops fill dolthub-remote-token` is the owner's one command. All four client-side routes are now measured and closed: privilege denied, server-side filesystem, no remotesapi, no dump client.

## C1 — bora-5ia: per-project section order

- [x] GC1: section order is declared per project and honored by the renderer.
  CHECK: cargo nextest run -E 'test(section_order)' --no-fail-fast 2>&1 | tail -3
  EXPECT: /[1-9][0-9]* passed/
  EVIDENCE: ──────────── | Summary [   0.026s] 9 tests run: 9 passed, 4007 skipped

- [x] GC2: a partial `order:` does not hide a declared section — unlisted ones still render, after the listed ones. This is the safety property; an order that silently swallows a band is worse than no ordering.
  EVIDENCE: `resolve_section_order` (project_view.rs:575) fills the array from the declaration, then runs a second loop over `ProjectSection::ALL` appending every variant not already placed — so the return is always a full 4-permutation, never a truncated list. Asserted by `section_order_resolve_partial_declaration_appends_unlisted_in_fixed_order`. The companion guarantee, that position cannot create visibility, is `section_order_listing_first_does_not_render_an_undeclared_section`: a project that never declares CHECKS renders no CHECKS band even with `checks` first in `order:`.

- [x] GC3: absent `order:` renders byte-identically to today.
  EVIDENCE: `resolve_section_order(None)` returns `ProjectSection::ALL` unchanged — the declaration loop is skipped and the append loop is a no-op fill in ALL order. Two tests pin it: `section_order_resolve_absent_matches_fixed_order` on the resolver, and `section_order_absent_matches_todays_default_rendered_sequence` end-to-end through `project_view_entries`. Also `section_order_to_yaml_round_trip_omits_absent_order`: a project without `order:` serializes without the key, so existing `projects.yml` files are untouched on rewrite.

- [x] GC4: mutation proof for GC1 and GC2 — revert each production change, show the named test fails, restore.
  EVIDENCE: Re-run by the integrator, not taken on the leaf's word. Stubbed `resolve_section_order` to `let _ = order; return ProjectSection::ALL;` (declaration ignored entirely) with a `cmp` guard proving the file changed: 5 of 9 tests FAILED — `section_order_resolve_unknown_name_ignored`, `section_order_resolve_full_declaration_matches_declared_sequence`, `section_order_resolve_duplicate_name_honored_once_at_first_position`, plus the partial-declaration and end-to-end wiring tests. The other 4 (absent-order, to_yaml, visibility) correctly kept passing, which is itself the right signal: they do not assert reordering. Restored, `cmp`-verified, 9/9 green. The leaf separately reported a second mutation (replacing `push_checks_section`'s `if declared.is_empty() { return; }` with `if false`) reddening the visibility test.

## C2 — bora-6ps: make the unwrap rule real

- [x] GC5: zero `clippy::unwrap_used` violations in production targets. Measured baseline before this work: 48, across 11 files (20 `src/api/wait.rs`, 10 `src/cli.rs`, 4 `src/cli/plugin.rs`, 3 `src/app/api/panes.rs`, 3 `src/app/api/tabs.rs`, 2 `src/app/api/responses.rs`, 2 `src/cli/agent.rs`, 1 each in `src/app/api/agents.rs`, `src/app/api.rs`, `src/layout.rs`, `src/protocol/render_ansi.rs`).
  CHECK: bash -c "touch src/main.rs && cargo clippy --bins --message-format json 2>/dev/null | python3 -c \"import sys,json; n=sum(1 for l in sys.stdin if l.startswith('{') and (json.loads(l).get('message') or {}).get('code') and (json.loads(l)['message']['code'] or {}).get('code')=='clippy::unwrap_used'); print('unwrap_used=%d' % n)\""
  EXPECT: unwrap_used=0
  EVIDENCE: unwrap_used=0

- [x] GC6: the rule is now machine-enforced, so it cannot silently rot again. Enforcement deliberately does NOT live in `Cargo.toml`'s `[lints.clippy]` — see the evidence, this gate was rewritten after the first attempt turned `just check` red.
  CHECK: bash -c "grep -c 'clippy::unwrap_used' justfile scripts/windows_check.ps1 | tr '\n' ' '"
  EXPECT: /justfile:2 scripts\/windows_check.ps1:2/
  EVIDENCE: The original gate demanded `unwrap_used = "deny"` in `Cargo.toml`, and that is what the leaf shipped — which made `just check` fail with a wall of `error: used unwrap() on a Result value`. Cause: `[lints.clippy]` has no per-target scope and `just lint` runs `cargo clippy --all-targets`, which compiles `#[cfg(test)]` modules, where this repo's fixtures unwrap by the hundred. Moved to the justfile, where the scope can be stated: `--all-targets` now carries `-A clippy::unwrap_used`, and a second `cargo clippy --bins --locked -- -D clippy::unwrap_used` run enforces the production scope AGENTS.md actually names, because `--bins` does not compile test modules. Mirrored in `scripts/windows_check.ps1`.

- [x] GC7: the enforcement instrument is not blind — injecting one production `unwrap()` must make the build fail.
  EVIDENCE: Injected `pub fn blind_probe() -> u8 { let x: Option<u8> = Some(1); x.unwrap() }` into `src/layout.rs` (production, not a test module), then `touch src/main.rs` to defeat clippy's cache. Result: `error: used `unwrap()` on an `Option` value` followed by `error: could not compile `bora` (bin "bora") due to 1 previous error`. A hard build failure, not a warning. Removed, `cmp`-verified, and the same command then finished clean. Note both traps this proof had to defeat: clippy does not re-emit warnings from a cached build, and `--message-format short` omits the lint name so grepping it for `clippy::unwrap_used` matches nothing — each produced a confident, wrong zero earlier in this task.

- [x] GC8: no violation was silenced with a bare `#[allow]`. Every remaining allow/expect carries a justification naming the invariant (AGENTS.md code conventions).
  EVIDENCE: `grep -rn "allow(clippy::unwrap_used)" src --include=*.rs` returns nothing — zero allows of any kind, bare or annotated. The 48 sites were removed rather than silenced: 39 serialization sites collapsed onto three single-owner encode helpers (6 of them deleted outright as literal duplicates of `print_response`'s error branch), and 9 invariant lookups became `expect()` naming what guarantees the value — e.g. `panes.rs:125`, where the pane was placed by `split_pane` and its terminal inserted on the immediately preceding line.

## Integration

- [x] GI1: `just check` green with both slices merged.
  CHECK: bash -c "just check 2>&1 | grep -E 'Summary|FAIL|^error' | head -3"
  EXPECT: /0 failed|tests run/
  EVIDENCE: Summary [  23.292s] 4015 tests run: 4015 passed (1 leaky), 1 skipped

- [x] GI2: beads reflect reality.
  CHECK: bash -c "bd list --all --json | jq -r '.[] | select(.id==\"bora-5ia\" or .id==\"bora-6ps\") | \"\(.id)=\(.status)\"' | sort | tr '\n' ' '"
  EXPECT: /bora-5ia=closed bora-6ps=closed/
  EVIDENCE: bora-5ia=closed bora-6ps=closed

- [x] GI3: version bumped and changelog entry written for the user-facing change (AGENTS.md Version Bump contract).
  CHECK: bash -c "grep -m1 '^version' Cargo.toml"
  EXPECT: /0\.42\.0/
  EVIDENCE: version = "0.42.0"

- [x] GI4: DOX pass — closest owning docs updated, or the reason left unchanged stated in the report.
  EVIDENCE: AGENTS.md Code Conventions: the `unwrap()` rule now states WHERE it is enforced and why not in `Cargo.toml`, plus the two measurement traps (clippy's warning cache and `--message-format short` dropping the lint name) as a dated binding rule. `docs/next/CHANGELOG.md` gained one Added entry for `sections.order:` — the user-facing half. `Cargo.toml` 0.41.0 -> 0.42.0. Deliberately unchanged: no changelog entry for bora-6ps (internal lint enforcement, not user-facing runtime behaviour, and the docs contract scopes the changelog to the latter) and none for the `windows_check.ps1` `--bin herdr` -> `--bin bora` fix (build tooling, same rule); the `sidebar-design.md` section-5 deferral is superseded by the dynamic-sidebar design question the owner just raised, so it is left for that design rather than marked RESOLVED against a shape that may change.
ABANDON lines go here, one per impossible gate, with the reason.
-->
