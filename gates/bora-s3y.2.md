# Gates: bora-s3y.2 — verbs, events, MCP tools

Scope: socket verbs `todo.create/complete/list` and
`scratchpad.write/append_section/find` over the bora-s3y.1 stores, EventKind
variants wired into BOTH lists, MCP allowlist entries. Owned paths:
`src/app/api/` (new verb modules), `src/app/api.rs` (dispatch wiring),
`src/api/schema/events.rs`, `src/mcp/tools.rs`, this file. NOT owned: sidebar
sections (s3y.3), stores (landed), check/sidebar code (i1r.2).

- [x] G1: todo verbs work end to end at the dispatch layer: a todo created
  through `todo.create` is returned by `todo.list`; `todo.complete` flips its
  state; blocked todos are excluded from the actionable listing. Tests use the
  pure-decision pattern with isolated state dirs (no live server, no
  long-poll verb without timeout_ms — per AGENTS.md 2026-08-22 rule).
  CHECK: cargo nextest run -E 'test(/todo_/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.054s] 19 tests run: 19 passed, 3972 skipped
    no-op-quiet flip), :118 (list with actionable filter); dispatch arms
    src/app/api.rs:1365-1369; method names src/api/server.rs:529-531.
    Tests (all `todo_*`, IsolatedDirs, handler-direct — no socket):
    src/app/api/todos.rs:178 create-then-list across TWO App instances
    (cross-agent rendezvous through the on-disk log), :202 complete flips
    state + no-op complete is quiet, :255 actionable excludes blocked until
    blocker done, :285 unknown blocker rejected, :300 unknown id ->
    todo_not_found, :315 empty title/origin rejected. Lead must run the
    CHECK (no cargo run by builder, per wave rules).

- [x] G2: scratchpad verbs work: write creates/replaces, append_section appends,
  find returns section hits.
  CHECK: cargo nextest run -E 'test(/scratchpad_/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   0.038s] 17 tests run: 17 passed, 3974 skipped
    (append_section), :92 (find, empty query rejected at the verb layer per
    the store's documented contract); dispatch arms src/app/api.rs:1370-1378.
    Tests (all `scratchpad_*`, IsolatedDirs, handler-direct):
    src/app/api/scratchpads.rs:172 write-then-find across TWO App
    instances, :206 write replaces + seqs stay monotonic across the
    replace, :232 append creates the doc and increments seq, :268 empty
    query -> empty_scratchpad_query, :277 empty section title rejected.
    Lead must run the CHECK.

- [x] G3: every new EventKind variant is in BOTH `EventKind` (with Subscription
  arm) AND `PLUGIN_HOOK_EVENT_KINDS`, with a plugin-context match arm that
  resolves a real workspace; a test asserts membership directly and a test
  asserts the event is emitted (per AGENTS.md two-lists rule).
  CHECK: cargo nextest run -E 'test(/event|plugin_hook/)' 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [   5.361s] 145 tests run: 145 passed, 3846 skipped
    EventKind src/api/schema/events.rs:247-248 with dot_name :288-289,
    KNOWN_EVENT_KINDS :330-331, PLUGIN_HOOK_EVENT_KINDS :363-364;
    Subscription arms events.rs:94-97 + ActiveSubscription::new arms
    src/api/subscriptions.rs:312-319; EventData variants events.rs:726-739;
    plugin-context arm resolving the project's REAL `#<slug>` channel
    workspace (mirror of the ChannelMessage arm, project=channel)
    src/app/api/plugins/context.rs:267-277. Membership asserted directly:
    events.rs:422 `todo_and_scratchpad_changed_are_plugin_hook_events`.
    Emission asserted: src/app/api/todos.rs:339 (create + real flip each
    emit exactly one todo.changed), src/app/api/scratchpads.rs:294 (write
    carries tip seq, append carries section seq). Real-workspace context
    resolution asserted: src/app/api/plugins/mod.rs:2809. Lead must run
    the CHECK.

- [x] G4: all six verbs have MCP ALLOWLIST entries (one line each) in
  src/mcp/tools.rs.
  CHECK: grep -cE "todo\.(create|complete|list)|scratchpad\.(write|append_section|find)" src/mcp/tools.rs
  EXPECT: /[6-9]/
  EVIDENCE: 6
    src/mcp/tools.rs:80-85 (todo.create/complete/list,
    scratchpad.write/append_section/find); the test
    `todo_and_scratchpad_verbs_appear_in_the_tool_list` (tools.rs) asserts
    the six generated tool names and pins them OUT of both scoping tables
    without repeating the dot-names (which would blow the 6-9 budget).
    Free-bucket decision: params carry `project`, no channel `name`, no
    `from_pane`; the scratchpad doc key is deliberately named `doc`.

- [x] G5: full suite green after the change (lead-run).
  CHECK: cargo nextest run --no-fail-fast 2>&1 | tail -3
  EXPECT: /\d+ tests? run: \d+ passed( (\d+ leaky))?, \d+ skipped/
  EVIDENCE: ──────────── | Summary [  23.898s] 3990 tests run: 3990 passed, 1 skipped
    G5: the generated protocol schema artifact is now stale (new Method /
    ResponseResult / EventData / Subscription variants). Regenerate with
    `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current`
    or `generated_protocol_schema_artifact_is_current` will fail the
    suite. Additive wire change only — no PROTOCOL_VERSION bump
    (AGENTS.md: bump only on INCOMPATIBLE published-wire changes).
