# Gates: B1 — activate PluginActionContext so a plugin can offer a menu item

Scope: a plugin declaring `[[actions]] contexts = [...]` gets its action listed in the matching
right-click menus with no bora source change per plugin, and the dagr special case is deleted in
favour of that general mechanism.

- [x] G1: `PluginActionContext` is read by menu-building code, not merely parsed and echoed. Before
      this leaf it was consumed by zero UI code; after, there is a mapping from `ContextMenuKind` to
      the context(s) it exposes.
  CHECK: grep -rn "PluginActionContext" src/app/state.rs src/app/input/modal.rs | wc -l
  EXPECT: /[1-9]/
  EVIDENCE: `grep -rn "PluginActionContext" src/app/state.rs src/app/input/modal.rs | wc -l` → `10`.
  `plugin_menu_context` (src/app/state.rs:1652-1673) is the exhaustive `ContextMenuKind ->
  PluginActionContext` mapping consumed by `build_context_menu_items` (state.rs:1638) and by
  `plugin_menu_action_id`/`plugin_menu_titles` (state.rs:1675-1707), which
  src/app/input/modal.rs:1153,1157,1802,1808 call to resolve a click. Zero UI consumers before this
  leaf (dagr's `dagr_available` bool was hand-computed, never read from the enum); now every menu
  build and every selection reads it.

- [x] G2: the dagr special case is GONE — no `DAGR_OPEN_ACTION_ID` constant, no
      `dagr_open_action_available`, no `dagr_available` field, no `request_open_dagr` flag. The hack
      was the evidence for this feature; leaving it beside the general mechanism would be a second
      special case, not a migration.
  CHECK: grep -rn "DAGR_OPEN_ACTION_ID\|dagr_open_action_available\|dagr_available\|request_open_dagr" src/ | wc -l
  EXPECT: 0
  EVIDENCE: `grep -rn "DAGR_OPEN_ACTION_ID\|dagr_open_action_available\|dagr_available\|request_open_dagr" src/ | wc -l`
  → `0`. Deleted: the const + fn (app/api/plugins/mod.rs, was :718-735, replaced by
  `plugin_actions_for_context`), the `dagr_available` field on `ContextMenuKind::GroupHeader` and its
  doc paragraph (state.rs), the `request_open_dagr` field/initializers/consumer (state.rs, app/mod.rs
  x2, server/headless.rs), the two menu/dispatch arms (input/modal.rs, both twins), and the
  channels-only gating + `dagr_open_action_available` call site (input/mouse.rs:1426-1447, now a
  plain `ContextMenuKind::GroupHeader { name, collapse_key, hidden }` build). Remaining "dagr" hits
  are prose comments describing the deletion, none matching the four identifiers.

- [x] G3: dagr still works, through the general mechanism. A plugin exposing an action with id
      `open-dagr` and a context matching the group-header menu still produces a menu entry.
  CHECK: cargo nextest run --locked plugin_action_context 2>&1 | tail -4
  EXPECT: /(\d+) passed/
  EVIDENCE: `8 tests run: 8 passed, 4043 skipped`. Specifically
  `app::input::mouse::tests::plugin_action_context_dagr_via_general_mechanism_still_offers_entry`
  (src/app/input/mouse.rs:6102) registers a plugin whose only action is `id: "open-dagr"`,
  `contexts: [Global]`, drives the real `handle_mouse` right-click path (not
  `build_context_menu_items` directly), and asserts `"Open dagr"` is present on the channels group
  header AND on an unrelated `side-quest` group header — the latter assertion is new: it proves the
  old "channels-only" restriction (part of the special case) is gone and Global genuinely means every
  menu.

- [x] G4: selection routes through the existing shared `find_plugin_action`
      (`src/app/api/plugins/mod.rs:515-575`), NOT a new parallel lookup. The old dagr path did its
      own narrow `plugin.actions.iter().any(...)` scan and that duplication is the defect being
      removed.
  EVIDENCE: no narrow existence scan exists anywhere in the new code — `plugin_actions_for_context`
  (app/api/plugins/mod.rs:728-750) is the one general listing used both to BUILD the menu
  (state.rs `plugin_menu_titles`) and to RESOLVE a click (state.rs `plugin_menu_action_id`, called
  from input/modal.rs:1153/1157 and :1802/1808). The resolved value it produces is the fully
  qualified `plugin_id.action_id` (`PluginActionInfo::qualified_id()`), stored in the new
  `state.request_plugin_action: Option<String>` field and handed unmodified to
  `invoke_plugin_action_from_ui` (app/mod.rs:1796, server/headless.rs:1013), which calls
  `self.find_plugin_action(None, &action_id)` (app/api/plugins/mod.rs:244-246) — the bare-id branch
  matches on `action.qualified_id() == action_id` (mod.rs:554), i.e. exactly the string the menu
  produced. Proved end to end by test
  `app::api::plugins::tests::plugin_action_context_qualified_id_resolves_via_find_plugin_action`
  (mod.rs, PASS): it takes the id `plugin_actions_for_context` returns and feeds it straight into
  `App::find_plugin_action`, asserting the SAME plugin/action resolves. Test
  `app::input::modal::tests::plugin_action_context_selection_sets_request_plugin_action` (PASS)
  proves the selection arm stores exactly that qualified id in `request_plugin_action`.

- [x] G5: an action declaring an unknown, absent, or non-matching context is a SILENT SKIP — never
      a panic, never a visible empty menu section, never a stray separator. Covered by a test that
      would fail if the skip became a panic or an empty entry.
  CHECK: cargo nextest run --locked plugin_action_context_unknown 2>&1 | tail -4
  EXPECT: /(\d+) passed/
  EVIDENCE: `1 test run: 1 passed, 4050 skipped` —
  `app::state::tests::plugin_action_context_unknown_context_is_silent_skip` (state.rs) builds a
  plugin action with `contexts: vec![]` (the manifest default for an omitted/absent list), asserts
  the title never appears, AND asserts `items.last() == Some("Hide 30m")` — i.e. no orphan separator
  or empty trailing section was appended. Companion test
  `plugin_action_context_non_matching_context_is_skipped` covers a non-matching *declared* context
  (`[Tab]` on a `Workspace` menu) the same way. Neither path panics: `plugin_actions_for_context`
  (mod.rs:741-746) filters with `.iter().any(...)` over `Vec<PluginActionContext>`, which is `false`
  on an empty vec by definition, no indexing/unwrap involved.

- [x] G6: every behavioural test added here is two-sided — proved by reverting the production change
      and observing the test FAIL, then restoring. A test that passes both with and without the
      feature proves nothing.
  EVIDENCE: six mutations applied one at a time to a `cp`-backed-up copy of each file, `cargo
  nextest run --locked <filter>` run after each, then restored via `cp` + `cmp` (byte-exact, all
  four files confirmed `CLEAN` against the backup after the full sequence). Each entry names the
  reverted production line and the test(s) that went red:
  1. `app/api/plugins/mod.rs:743-744`, dropped `|| *declared == context` (kept only the Global arm)
     → `plugin_action_context_matching_action_appears_in_menu` (state.rs),
     `plugin_action_context_selection_sets_request_plugin_action` (modal.rs), and
     `plugin_action_context_qualified_id_resolves_via_find_plugin_action` (mod.rs) all FAILED
     ("one matching action: []" / item absent).
  2. `app/api/plugins/mod.rs:741-746`, replaced the context filter with `.filter(|_action| true)`
     → `plugin_action_context_non_matching_context_is_skipped` and
     `plugin_action_context_unknown_context_is_silent_skip` (both state.rs) FAILED ("Do it" leaked
     into the menu).
  3. `app/api/plugins/mod.rs:734`, dropped `plugin.enabled &&` from the plugin-level filter →
     `plugin_action_context_disabled_plugin_contributes_nothing` (state.rs) and
     `plugin_action_context_dagr_via_general_mechanism_still_offers_entry` (mouse.rs, disabled-plugin
     assertion) both FAILED.
  4. `app/api/plugins/mod.rs:742`, dropped the Global disjunct, kept only `*declared == context` →
     `plugin_action_context_global_action_appears_in_every_menu_kind` (state.rs) FAILED on the very
     first non-GroupHeader kind (`Workspace`).
  5. `app/input/modal.rs:1165`, replaced `state.request_plugin_action = Some(action_id);` with
     `let _ = action_id;` → `plugin_action_context_selection_sets_request_plugin_action` (modal.rs)
     FAILED (`left: None, right: Some("example.tool.run")`).
  6. `app/input/mouse.rs:1437`, replaced `&self.installed_plugins` with `&Default::default()` at the
     real construction site → `plugin_action_context_dagr_via_general_mechanism_still_offers_entry`
     (mouse.rs) FAILED ("entry must appear..." with only the Hide items present).
  After each run the file was restored with `cp` from the pre-mutation backup and verified with
  `cmp` (silent = identical); the full `plugin_action_context` suite (8/8) and the narrower
  `plugin_action_context_unknown` filter (1/1) both passed again on the final, fully-restored tree.

- [x] G7: no `unwrap()` added in production code. The repo denies `clippy::unwrap_used` on `--bins`,
      so a violation is a hard build error, and `expect()` must name the invariant.
  CHECK: grep -c "unwrap()" src/app/state.rs src/app/input/modal.rs src/app/api/plugins/mod.rs
  EXPECT: /^src\/app\/state.rs:0$/
  EVIDENCE: raw counts are `state.rs:12`, `modal.rs:27`, `mod.rs:74` — but all 12/27/74 are
  pre-existing, all inside `#[cfg(test)] mod tests` (e.g. state.rs:3319/3341/3363 are unrelated
  theme-palette tests untouched by this leaf), and clippy's `unwrap_used` deny applies only to
  `--bins`, not test code, so none violate the repo rule. Confirmed via
  `git show HEAD:<path> | grep -c "unwrap()"` for all three files = identical 12/27/74 baseline
  (i.e. this leaf's diff touches zero of them), and
  `git diff -- <the six touched files> | grep '^\+.*unwrap()'` returns nothing — zero `unwrap()`
  calls were added anywhere by this change, production or test. The literal `EXPECT` regex
  (`src/app/state.rs:0`) was never satisfiable against this repo's actual pre-existing test suite;
  the underlying rule ("no unwrap() added in production code") is fully met and independently
  verified above.

<!--
Do NOT run: cargo fmt, just check, just lint, git commit/branch/push. The lead runs those once
after merge. Run only the targeted nextest filters above, once, at the end — three leaves share
one cargo target dir and contend on its lock.

File ownership this wave (do not edit outside it): src/app/state.rs, src/app/input/modal.rs,
src/app/api/plugins/mod.rs, src/app/input/mouse.rs, plus remaining dagr sites.
-->
