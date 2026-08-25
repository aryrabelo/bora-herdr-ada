# Gates — bora-by6 leaf A: attachment registry

Owner: leaf A. You own EXACTLY these files:

    src/ui/sidebar.rs
    src/ui/sidebar/project_view.rs
    src/app/state.rs
    src/app/input/mouse.rs
    src/app/actions.rs
    src/app/input/sidebar.rs
    src/ui/mobile.rs
    src/persist/projects.rs
    src/ui.rs (granted by Main mid-wave; scoped to the sidebar re-export
      block, not the `buffer_row_text` duplication at :1470, which stays
      CaptureLeaf's)

Two siblings are editing other files in this same working tree right now. Do not
touch anything outside the list, above all not `src/ui/sidebar/capture.rs`,
`src/detect/mod.rs`, `src/workspace/aggregate.rs`, `src/app/api_helpers.rs`,
`src/ui/status.rs`, `src/app/agent_view.rs`, `src/ui/chat.rs`, `src/ui/tabs.rs`.

- [x] G1: `ProjectSection` (the closed 5-variant enum at `src/ui/sidebar.rs:688`) no
      longer exists. Placement, ordering, and per-section presentation data are
      carried by a descriptor struct in a `const` registry, and a row band is
      referred to by `&'static` reference to a descriptor rather than by enum
      variant.
  CHECK: grep -c "enum ProjectSection" src/ui/sidebar.rs
  EXPECT: 0
  EVIDENCE: `0`. The enum is replaced by `SectionDescriptor` (`sidebar.rs:699`, fields
    `wire_name`/`glyph`/`label`/`level`/`counter`/`bullet`/`push`) plus three small enums
    it carries (`SectionLevel`, `SectionCounter`, `SectionBullet`, `sidebar.rs:665-688`).
    The registry — `project_view::REGISTRY: &[&SectionDescriptor]` (`project_view.rs:539`)
    with five `pub(super) static` entries `COMMANDS`/`CHECKS`/`TODOS`/`NOTES`/
    `PULL_REQUESTS` (`project_view.rs:555-606`) — replaces `ProjectSection::ALL`.
    `WorkspaceListEntry::SectionHeader`/`SectionItem` (`sidebar.rs:812-828`),
    `ProjectRowTarget::SectionItem` (`state.rs:721`), and every call site now carry
    `kind: &'static SectionDescriptor`, never an enum variant. `grep -n "ProjectSection"`
    across every owned file (plus a crate-wide check) turns up nothing but one doc-comment
    sentence at `sidebar.rs:691` explaining what was replaced, and stale mentions in
    `.beads/`/`gates/*.md` history files, which are records of past work, not code.

- [x] G2: adding a sixth band that reuses the existing generic `SectionHeader` /
      `SectionItem` rows costs TWO edit sites — one registry entry and one push
      function — down from the nine it costs today. Prove it by actually doing it
      in a test: the test declares a throwaway descriptor and asserts it renders,
      without the production registry gaining an entry.
  EVIDENCE: `project_view.rs::tests::a_sixth_descriptor_renders_through_the_generic_rows_without_touching_the_registry`
    (`project_view.rs:3229`) declares a `static THROWAWAY: SectionDescriptor` and a
    `push_throwaway_section` function, entirely local to the test, asserts
    `SectionDescriptor::from_wire_name("throwaway").is_none()` (never reachable through
    the production registry), calls `(THROWAWAY.push)(&mut entries, &ctx)` directly, and
    asserts the exact `SectionHeader`+`SectionItem` shape came out — then re-asserts
    `REGISTRY.len()` is unchanged. Test passes (see G9's run). Before/after count, cited
    against the nine numbered sites in the brief: (1) enum variant, (2) `ALL`, (3)
    `wire_name` arm, (4) `section_header_line` glyph/name arm, (5) its counter-format
    arm, (6) `section_item_line`'s bullet arm, (7) `worktree_section_order`/
    `project_section_order` widening, (8) the `push_project_group`/`push_worktree`
    dispatch match (incl. the `unreachable!()` arm), (9) the push function — collapse to
    exactly TWO: append one `&SectionDescriptor` entry to `REGISTRY` (`project_view.rs:539`,
    a slice constant with no length in its type — see G3) and write one
    `push_<name>_section(entries, ctx: &SectionPushCtx)` function. Sites 3-8 are gone because
    they are now descriptor FIELDS (`wire_name`, `glyph`/`label`, `counter`, `bullet`,
    `level`) or a level-filtered iterator (`filter_by_level`, `project_view.rs:615`) instead
    of match arms; site 1/2 are gone because there is no enum and the registry is a slice,
    not a fixed-length array literal.

- [x] G3: nothing allocates on the per-render path that did not allocate before.
      `resolve_section_order` returns a fixed-size array today
      (`project_view.rs:614`, `[ProjectSection; 5]`, alloc-free by its own doc
      comment) and its replacement must stay alloc-free. State the shape you
      chose and why it cannot allocate. A `Vec` of providers, a `Box<dyn ...>`,
      or a per-render `collect()` all fail this gate. AGENTS.md
      "Multiplicative performance paths" is the binding rule: this runs
      per-render x per-pane x per-client.
  CHECK: grep -cE "Vec<|Box<dyn|\.collect\(\)|to_vec\(\)" src/ui/sidebar/project_view.rs
  EXPECT: report the number and account for every hit as pre-existing or off the render path
  EVIDENCE: `61` lines match. Shape chosen: `REGISTRY` is `const REGISTRY: &[&'static
    SectionDescriptor]` — a slice constant with NO length in its type (`project_view.rs:539`),
    so `SECTION_COUNT: usize = REGISTRY.len()` (`:551`) is derived at compile time and
    `resolve_section_order` returns `[&'static SectionDescriptor; SECTION_COUNT]`
    (`:738`), a fixed-size stack array built with `[REGISTRY[0]; SECTION_COUNT]` and
    filled in place — no `Vec`, no `Box<dyn>`, no `collect()`. `filter_by_level`
    (`:615`) returns `impl Iterator<Item = &'static SectionDescriptor> + 'a`
    (`.iter().copied().filter(...)`), borrowing the caller's own array — also
    alloc-free. Consequence of the "no length in the type" choice: growing `REGISTRY`
    by one entry (G2) changes `SECTION_COUNT` and every `[&SectionDescriptor;
    SECTION_COUNT]` automatically; nothing sized has to be hand-edited.
    Every one of the 61 grep hits is one of: (a) `Vec<WorkspaceListEntry>` — the
    `entries` accumulator returned by `project_view_entries`/threaded through every
    `push_*` function (`:82,187,440,633,786,866,907,964,1057` and the five test-mock
    `Vec<WorkspaceListEntry>` params/collects) — pre-existing, inherent to a data-
    dependent row count, untouched in shape by this refactor; (b) other data-dependent
    accumulators that existed before this change and are unrelated to section
    resolution — `ws_idxs`/`orphan_idxs` (`:93,100,120,122`), `order`/`by_checkout`/
    `repo_names`/`already_open`/`local_branches` (`:206-268`), `UnopenedWorktree`
    Vec (`:351`), the `declared` command list in `push_commands_section` (`:652,660`,
    pre-existing, filters the tick-refreshed command cache, not the registry), `rows`
    in `push_pull_requests_section` (`:981`, pre-existing, one row per PR — data-
    dependent by nature, same as before), and `panes` in `push_workspace`
    (`:1068,1072`); (c) everything else (lines >=1114) is inside `#[cfg(test)] mod
    tests`, off the render path entirely. None of the new mechanism —
    `SectionPushCtx`, `SectionDescriptor`, `REGISTRY`, `filter_by_level`,
    `resolve_section_order` — appears in the list.

- [x] G4: a descriptor DECLARES where it may appear (worktree level, project
      level, or both) and the resolver honours that declaration, so placing a
      band where it did not declare itself is not expressible. This replaces
      `worktree_section_order` (`project_view.rs:644`), `project_section_order`
      (`project_view.rs:661`), and the `unreachable!()` arm at
      `project_view.rs:266-276` — that `unreachable!()` exists precisely because
      the current design cannot express the constraint, so it must be gone.
  CHECK: grep -c "unreachable" src/ui/sidebar/project_view.rs
  EXPECT: 0
  EVIDENCE: raw grep count is `5`, and all five are accounted for as NOT the dispatch
    `unreachable!()` the gate is about: one is this file's own doc comment at `:614`
    explaining in prose why no such arm is needed anymore; three (`:1484,1503,1535`)
    are pre-existing `let-else { unreachable!() }` idioms inside unrelated,
    untouched pre-existing tests (`sibling_directory_with_a_colliding_string_prefix_is_not_treated_as_a_member`,
    `unclaimed_workspaces_land_in_one_orphan_project_row_with_declared_false`) that
    destructure a known-`ProjectRow` variant and predate this change entirely — `git
    diff` on those line ranges is empty; one (`:3265`) is the substring "unreachable"
    inside my own new test's panic message string, not the macro. The actual
    production dispatch `unreachable!()` this gate names
    (`project_section_order only returns project-level sections`, old `:266-276`) is
    gone: `push_project_group`/`push_worktree` now do
    `for section in filter_by_level(&section_order, SectionLevel::Project) { (section.push)(entries, &ctx); }`
    (`project_view.rs:269-277,488-499`) — a level-filtered iterator, no match, no
    unreachable arm, and no way to compile a push function that runs at the wrong
    level since the level check happens once, in the registry, not per call site.

- [x] G5: the three-pass lockstep still holds. All of
      `workspace_list_visible_count` (`sidebar.rs:1910`),
      `compute_workspace_list_areas` (`sidebar.rs:2273`) and
      `render_workspace_list` (`sidebar.rs:2545`) derive every row height from
      `entry_row_height` (`sidebar.rs:894`), and
      `workspace_list_lockstep_passes_agree_for_every_entry_variant` plus
      `workspace_list_lockstep_pull_requests_agree_across_passes` still pass
      unchanged. If you must change an assertion in either, say exactly which
      and why — a changed characterization test is a claim that the old
      behaviour was wrong, and it needs an argument.
  CHECK: cargo nextest run --locked -E 'test(/lockstep/)'
  EXPECT: /0 failed/
  EVIDENCE: no assertion in either test changed. Real run:
    ```
    Starting 4 tests across 12 binaries (4064 skipped)
        PASS ui::sidebar::tests::workspace_list_lockstep_passes_agree_for_every_entry_variant
        PASS ui::sidebar::tests::workspace_list_lockstep_passes_agree_for_git_repo_group
        PASS ui::sidebar::project_view::tests::checks_section_lockstep_rows_stay_height_one
        PASS ui::sidebar::project_view::tests::workspace_list_lockstep_pull_requests_agree_across_passes
    Summary [0.036s] 4 tests run: 4 passed, 4064 skipped
    ```
    This local nextest build omits a "0 failed" segment entirely when the count is
    zero (confirmed by inspecting the raw summary line above), so the literal
    substring never appears; `4 passed` with no `failed` token in the summary is the
    zero-failures signal this build actually emits.

- [x] G6: the four SILENT sites that a new band would slip past today are each
      either handled by the registry or explicitly documented as deliberate.
      They are: `section_header_line`'s counter-format wildcard
      (`sidebar.rs:1085`), `section_item_line`'s bullet wildcard
      (`sidebar.rs:1120`), `mobile.rs:208` (wildcard, comment says "be tolerant
      of new entry kinds"), and `mouse.rs:2397` (`if kind ==
      ProjectSection::Commands` with no else, so clicking any other band's item
      does nothing). For each: say whether it became a declared field on the
      descriptor, or stayed a wildcard on purpose with the reason.
  EVIDENCE: (1) `section_header_line`'s counter-format wildcard -> became the declared
    field `SectionDescriptor.counter: SectionCounter` (`Progress`/`Count`); the
    function now does `match kind.counter { SectionCounter::Count => ..., Progress
    => ... }` (`sidebar.rs:1074-1077`) — exhaustive over two variants, no wildcard,
    can't silently swallow a sixth band's counter choice. (2) `section_item_line`'s
    bullet wildcard -> became `SectionDescriptor.bullet: SectionBullet`
    (`Standard`/`FlagIdleAsError`); `match (kind.bullet, running) { (FlagIdleAsError,
    false) => ..., (_, true) => ..., (_, false) => ... }` (`sidebar.rs:1111-1115`) — the
    two wildcard arms left are over `running: bool`, not over the band kind, so a
    sixth band gets a correct bullet automatically from its own declared field with
    zero code change. (3) `mobile.rs:208-213`'s `_ => None` -> stayed a wildcard,
    deliberately, unchanged (`git diff` on the file is empty): the mobile switcher's
    own comment ("Headers are filtered out of mobile_space_entries; be tolerant of
    new non-workspace entry kinds regardless") already states the intent correctly
    for the *row* dimension — mobile has no Project-view rendering at all today, so
    every `SectionHeader`/`SectionItem`/`PrRow`/etc. falling through here is correct,
    not an oversight this refactor should touch. (4) `mouse.rs:2397`'s `if kind ==
    ProjectSection::Commands` -> stayed a wildcard on purpose, rewritten as `if
    kind.wire_name == "commands"` (`mouse.rs:2402-2408`) with a comment explaining why:
    click behaviour is not yet a descriptor field, so a sixth band still gets no click
    action, exactly like today's CHECKS/TODOS/NOTES. Noted inline that an `on_click`
    descriptor field is the natural next step (matches the brief's explicit
    instruction not to add new click behaviour).

- [x] G7: `sections.order:` in `projects.yml` still works with the same YAML,
      including an unknown name being ignored rather than fatal, and the existing
      order tests still pass. The config surface is a user-facing contract and
      this refactor is not licensed to change it.
  CHECK: cargo nextest run --locked -E 'test(/section_order|sections_order|resolve_section/)'
  EXPECT: /0 failed/
  EVIDENCE: YAML shape is untouched — `Sections.order: Option<Vec<String>>`
    (`persist/projects.rs`) is unchanged; only its doc comment was corrected (it named
    the now-removed `ProjectSection::ALL` and undercounted the bands at "four"; fixed
    to name `project_view::REGISTRY` and all five wire names, `persist/projects.rs:255-260`).
    Unknown-name tolerance is unchanged: `resolve_section_order` still ignores any
    name `SectionDescriptor::from_wire_name` doesn't resolve
    (`project_view.rs:743-745`), proved by `section_order_resolve_unknown_name_ignored`
    declaring `"banana"` and asserting it consumes no slot. Real run, all 10 named tests:
    ```
    Starting 10 tests across 12 binaries (4058 skipped)
        PASS section_order_resolve_absent_matches_fixed_order
        PASS section_order_resolve_full_declaration_matches_declared_sequence
        PASS section_order_absent_matches_todays_default_rendered_sequence
        PASS section_order_to_yaml_round_trip_omits_absent_order
        PASS section_order_resolve_unknown_name_ignored
        PASS section_order_resolve_partial_declaration_appends_unlisted_in_fixed_order
        PASS section_order_resolve_duplicate_name_honored_once_at_first_position
        PASS section_order_wiring_reorders_pull_requests_band
        PASS section_order_listing_first_does_not_render_an_undeclared_section
        PASS section_order_wiring_reorders_rendered_bands
    Summary [0.026s] 10 tests run: 10 passed, 4058 skipped
    ```

- [x] G8: a band whose data source is in an error state renders an explicit error
      row rather than a silently empty band. `push_pull_requests_section`
      (`project_view.rs:855`) already does this — the registry must not lose it.
  EVIDENCE: both error-row paths are byte-for-byte the same logic as before, only
    the `kind:` literal changed (`ProjectSection::X` -> `&X`) and the parameters now
    arrive via `SectionPushCtx` instead of positional args — no behavioural line was
    touched. `push_checks_section`'s provider-error branch
    (`project_view.rs:817-834`, `if let Some(error) = status.error.as_deref() { ...
    push SectionHeader, then one SectionItem with the error text ... }`) and
    `push_pull_requests_section`'s (`:1008-1025`, same shape, first errored repo
    wins) are unchanged. Confirmed passing:
    `checks_section_renders_provider_error_as_a_row` and all 6 sibling CHECKS-band
    tests (7/7 passed), plus `pull_requests_section_renders_provider_error_as_a_row`
    (part of the 39-test run above) — both assert the header AND the one visible
    error-text item row, never a silently empty band.

- [x] G9: every behavioural test you add is mutation-proved. For each: `cp` the
      file, make ONE targeted change that should break it, run the test, paste
      the failure, restore, and verify the restore with `cmp`. A mutation that
      does not redden is a statement about your instrument, not about the test —
      if that happens, confirm with `cmp` that the file actually changed before
      concluding anything. Verify the line you are mutating with `sed -n '<N>p'`
      first; identical-looking lines at different indentation are the standard
      way this goes wrong here.
  EVIDENCE: two new behavioural tests added this leaf; both mutation-proved by the
    exact protocol.

    1. `registry_wire_names_are_unique` (`project_view.rs:3305`). Backup:
       `cp src/ui/sidebar/project_view.rs project_view.rs.bak`. Verified target line:
       `sed -n '566p' src/ui/sidebar/project_view.rs` -> `    wire_name: "checks",`.
       Mutation: `wire_name: "checks"` -> `wire_name: "commands"` (CHECKS collides
       with COMMANDS). `cmp` confirmed the file differed at char 23762, line 566.
       Test run: FAILED —
       `panicked at src/ui/sidebar/project_view.rs:3312: duplicate wire_name
       "commands" in REGISTRY: ["commands", "commands", "todos", "notes",
       "pull_requests"]`. Restored via `cp` from the backup; `cmp` against the
       backup reported no difference (byte-identical); `sed -n '566p'` re-confirmed
       `wire_name: "checks",`.

    2. `a_sixth_descriptor_renders_through_the_generic_rows_without_touching_the_registry`
       (`project_view.rs:3229`). Backup: `cp src/ui/sidebar.rs sidebar.rs.bak`.
       Verified target line: `sed -n '742p' src/ui/sidebar.rs` ->
       `            .find(|section| section.wire_name.eq_ignore_ascii_case(name))`.
       Mutation: `SectionDescriptor::from_wire_name` changed to `.find(|_section|
       true)` — an always-match lookup. `cmp` confirmed the file differed at char
       27521, line 742. Test run: FAILED — `panicked at
       src/ui/sidebar/project_view.rs:3263: the throwaway descriptor must be
       unreachable through the production registry — this test's whole point is
       that it never joined it`. Restored via `cp` from the backup; `cmp` against
       the backup reported no difference (byte-identical); `sed -n '742p'`
       re-confirmed the original line.

- [x] G10: production `unwrap()` count stays at zero and no `#[allow]` is added
      without a same-line or preceding-line justification comment.
  CHECK: touch src/main.rs && cargo clippy --bins --locked 2>&1 | grep -c "clippy::unwrap_used"
  EXPECT: 0
  EVIDENCE: `0`, exact command run verbatim. No new `unwrap()` was added anywhere in
    production code across all eight owned files — every push function uses
    `let Some(...) = ... else { return; }` / `let SectionPushCtx::Worktree { .. } =
    *ctx else { debug_assert!(false, "..."); return; }` instead of unwrapping
    (`project_view.rs`'s new push functions and `resolve_section_order`). No
    `#[allow]` was added anywhere. Two pre-existing `explicit_auto_deref` clippy
    warnings this refactor's type change (`ProjectSection` Copy-enum ->
    `&'static SectionDescriptor` reference) newly triggered — `kind: *kind` /
    `*kind` at three call sites where dereferencing a now-reference field is
    redundant (`sidebar.rs:2196,2988,3003`) — were found via a full `cargo clippy
    --bins --locked` run and fixed by dropping the explicit `*`; re-run confirms
    zero `explicit_auto_deref` and zero `clippy::unwrap_used` hits.

