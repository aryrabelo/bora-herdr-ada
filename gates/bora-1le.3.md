# Gates — bora-1le.3 — dagr + herdr-plus detection hooks

Lead dispatch did not include a pre-created gates file; ledger created by the
builder from the bead's verbatim acceptance criteria. Evidence references the
final working tree. `cargo` was not run (dispatch forbids it) — the lead runs
the gates.

Bead verbatim: "herdr-dagr: run_file convention .bora/run.json; project
right-click 'open dagr' via plugin.action.invoke open-dagr; silent skip when
absent. herdr-plus: defaults.open_with may be 'herdr-plus open <template>';
per-member template: field; fall back to bora's open when absent. Never a hard
dependency (settled decision #10)."
Acceptance: "'open dagr' appears only when installed; open_with template path
exercised; absence degrades clean."

## Gate 1 — 'open dagr' appears only when installed (BOTH directions)

Surface decision, grounded before coding: no project row / `ContextMenuKind`
exists yet (bora-1le.1 landed channel binding only; the e9i project-view
sidebar bead is later). Per settled decision #8 (project = channel), the
project surface TODAY is the channels group header (`vg:<channel_group_name>`),
confirmed with the ProjectChannel builder over hub before touching anything.

- Menu decision is pure: `ContextMenuKind::GroupHeader` grew
  `dagr_available: bool` (src/app/state.rs:1301-1306); the arm builds
  `"Open dagr"` after a separator only when it is `true`
  (src/app/state.rs:1508-1527).
- Detection is registry-driven, never a filesystem probe:
  `dagr_open_action_available` (src/app/api/plugins/mod.rs:722-735) = some
  plugin in the registry is enabled + manifest-available + declares the
  `open-dagr` action (`DAGR_OPEN_ACTION_ID`, src/app/api/plugins/mod.rs:718).
- Gated to the project surface at the single construction site
  (src/app/input/mouse.rs:1427-1447): `dagr_available` is computed only when
  `collapse_key == "vg:{channel_group_name}"` — a repo header or an arbitrary
  visual group never gets the entry.
- Invoke path: choosing the entry sets `request_open_dagr`
  (both modal twins, src/app/input/modal.rs:1142-1153 and :1797-1809); the App
  loop and the headless server loop drain it into
  `invoke_plugin_action_from_ui(DAGR_OPEN_ACTION_ID, "sidebar")`
  (src/app/mod.rs:1739-1751, src/server/headless.rs:1013-1025), which
  re-refreshes the registry and runs the plugin command — the same
  `plugin.action.invoke` mechanics the keybind path uses, refactored out of
  `invoke_plugin_action_from_keybind` (src/app/api/plugins/mod.rs:225-267).
- EVIDENCE (both directions):
  - src/app/state.rs:3372-3415
    `group_header_menu_open_dagr_entry_tracks_availability_both_ways`:
    present+separator when `dagr_available: true`; absent AND no orphan
    separator when `false`. Goes red if the arm always-pushes (present assert
    on the false build fails) or never-pushes (present assert on the true
    build fails).
  - src/app/input/mouse.rs:5973-6057
    `right_click_channels_group_header_offers_open_dagr_only_when_registered`:
    four scenarios through the real handler — registered+enabled (entry +
    `dagr_available: true` on the kind), other visual group (no entry even
    with the plugin), disabled install (no entry), no plugin at all (no entry,
    menu still opens — silent skip, no error path taken).

## Gate 2 — open_with template path exercised

- Schema: `Member.template: Option<String>` (src/persist/projects.rs:176-182),
  added AFTER ProjectChannel reported its own projects.rs edit done (hub
  coordination, per dispatch contract). Opt-in per member, `deny_unknown_fields`
  makes the key a parse error before the field exists.
- Decision: `resolve_open_with(defaults, member, opener_available)`
  (src/persist/projects.rs:191-219) — substitutes member `template:` into the
  `<template>` placeholder of `defaults.open_with`, uses the result only when
  the caller reports the named opener program available. Detection stays the
  caller's concern so the decision stays pure/testable.
- EVIDENCE: src/persist/projects.rs:754-777 `member_template_field_parses_and_defaults_to_absent`
  (schema lock: `template:` key parses, absent defaults to None) and
  :779-802 `open_with_template_path_is_exercised_when_opener_available`
  (`"herdr-plus open <template>"` + member `template: web` + available →
  `"herdr-plus open web"`; no member → placeholder left for the opener to
  interpret, bora invents no default template).

## Gate 3 — absence degrades clean

- Opener absent → `resolve_open_with` returns bora's own
  `default_open_with()` ("bora workspace open"), a plain `String` — no
  Result, no error to surface, no partial fallback (fallback contains no
  reference to the missing opener). EVIDENCE:
  src/persist/projects.rs:804-833 `open_with_missing_opener_falls_back_to_bora_open`
  (also pins the empty-open_with edge).
- dagr absent → no menu entry is built at all: no greyed-out row, no toast, no
  log line. The only logging on the dagr path is ONE `tracing::warn!` in the
  App/headless consumer when a SHOWN entry's invoke fails at click time
  (plugin removed between menu-open and click) — a user action that failed,
  not absence noise. EVIDENCE: mouse test scenario 4 (menu opens normally,
  no entry) + the invoke re-check comment at src/app/mod.rs:1739-1751.
- Never a hard dependency (decision #10): the sidebar renders identically
  with an empty plugin registry (`dagr_open_action_available(&HashMap::new())`
  is just `false` — no I/O, no panic path); `resolve_open_with` is a pure
  function consulted at open time. Nothing in the render path reads either
  tool.

## Gate 4 — tests go red without the change

- Menu test (state.rs): remove the `if *dagr_available` push → present-assert
  fails; make it unconditional → absent-assert fails. Both mutations caught.
- Mouse test: delete the channels-group gate (show on every header) →
  side-quest scenario fails; delete the availability call (hardcode true) →
  disabled/absent scenarios fail; hardcode false → registered scenario fails.
- Modal test: drop the arm → falls to catch-all → `request_open_dagr` stays
  false → assert fails.
- Persist tests: remove the fallback branch → absent-opener assert fails;
  remove substitution → template-path assert fails; remove the field → parse
  test fails via `deny_unknown_fields`.
- Blind-value check performed on each: every assertion names the exact string
  ("Open dagr", "herdr-plus open web", "bora workspace open") — no `contains`,
  no counting, no assertion satisfiable by neighbouring behaviour.

## Gate 5 — build/test/lint green

- EVIDENCE: pending — lead runs it (builder may not run cargo).

## Gate 6 — docs/next/CHANGELOG.md

- One bullet appended under the existing `## Unreleased` → `### Added`:
  docs/next/CHANGELOG.md:16. Root CHANGELOG.md untouched.

## Contract notes / corrections

- None to the phase doc: `.local/prd/sidebar-design.md` decisions #8/#9/#10
  were followed as written. The PRD's "project right-click" lands on the
  channels group header because that is the only project surface in the tree
  today (confirmed with ProjectChannel); the e9i project-view bead moves the
  entry when its row exists.
- `Orchestrator.run_file` and `defaults.open_with` already existed in the
  schema (verified before editing, per dispatch); only `Member.template` was
  added.
