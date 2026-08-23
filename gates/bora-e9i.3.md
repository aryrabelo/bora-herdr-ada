# Gates — bora-e9i.3 — MCP exposure of project verbs

Lead dispatch did not include a pre-created gates file; ledger created by the
builder from the bead's verbatim acceptance criteria. Evidence references the
final working tree. `cargo` was not run (dispatch forbids it) — the lead runs
the gates.

## Gate 1 — One ALLOWLIST line per project verb
- Five entries, no name overrides (`project.list`/`.create`/`.update`/
  `.member_add`/`.member_remove` → `project_list`/`project_create`/
  `project_update`/`project_member_add`/`project_member_remove`):
  src/mcp/tools.rs:70-74.
- No hand-written schema: `build_tools` walks `schema_for!(Method)`'s oneOf
  variants (src/mcp/tools.rs:132-134 after edit), so each tool's inputSchema
  comes from the params structs' `schemars` derives; the `Method` variants
  exist (src/api/schema.rs:280-288) and their wire names are pinned in
  `api_method_name` (src/api/server.rs:524-528).
- `every_tool_schema_is_self_contained` (existing test) automatically covers
  the new tools' `$ref` inlining, including `ProjectMemberAddParams`'
  `WorktreesScope` `$ref`.

## Gate 2 — `tools/call project_member_add` mutates the file
- MCP adds no new mutation path: `dispatch` translates the tool name to its
  wire method (src/mcp/tools.rs:314-387 region, unchanged this bead) and sends
  the same `Request` the CLI sends; the write itself — fresh read, atomic
  tmp+rename — is bora-e9i.2's landed and tested behavior
  (src/app/api/projects.rs handlers, src/persist/projects.rs
  `update_projects_file`). Exposure ≠ re-implementation: the allowlist line is
  the entire change that makes the verb callable over MCP.
- The end-to-end assertion is the lead's runtime check (start `bora mcp serve`,
  call `project_member_add`, diff `~/.config/bora/projects.yml`). Per the
  AGENTS.md 2026-08-22 socket rule, no unit test here may perform it: the verb
  writes the real projects.yml of whatever bora server is live on this machine.
- EVIDENCE: pending — lead runs the live check.

## Gate 3 — Scoping analysis, answered per verb in a comment
The last wave's defect class; answered from what each verb lets the caller
OBSERVE, in the audit comment at src/mcp/tools.rs:45-69:
- `project.list` (`EmptyParams`): no channel, no `from_pane`.
- `project.member_add` (`slug`,`dir`,`worktrees`): no channel, no `from_pane`.
- `project.member_remove` (`slug`,`dir`): no channel, no `from_pane`.
- `project.create` / `project.update`: channel YES — top level, under
  `channel` (never `name`). Decision: NOT a fence escape, deliberately
  unfenced. Reasons, verified against the tree: (a) a fence protects channel
  TRAFFIC and every traffic verb takes its own already-fenced top-level
  `name`; (b) `effective_channel`'s only production caller is
  `project_summary` (src/app/api/projects.rs:44), which echoes the caller's
  own string into the response — nothing joins, subscribes, or reads any
  channel's messages off the stored string, so unlike `events_wait`'s nested
  channel the caller observes only its own input; (c) fencing would be
  mechanically wrong anyway — the fence reads `name`, which on these verbs is
  the project DISPLAY name, so every project not named after a channel slug
  would be rejected. Residual disclosure stated honestly: `project.list`
  shows which channel other projects point at — config metadata, same class
  as project names/member dirs these verbs expose by design, not live
  channel state like the filtered `channel_list`.
- `from_pane`: none of the five params structs carries one (stated in the
  FROM_PANE_TOOLS doc comment, src/mcp/tools.rs:103-108).
- Both scoping tables' doc comments state the project verdict with the why
  (src/mcp/tools.rs:77-91, :103-108).

## Gate 4 — Presence test goes red without the change
- `project_verbs_appear_in_the_tool_list` (src/mcp/tools.rs:586-607): explicit
  list of all five `(tool, wire)` pairs asserted against `tool_index(true)` by
  name. Remove any single ALLOWLIST line → `build_tools` skips that Method
  variant → `index.get(tool)` is `None` → assert fails. No counting: a count
  would pass with four of five.

## Gate 5 — Scoping decision pinned so it cannot silently move
- `project_verbs_belong_to_no_scoping_table` (src/mcp/tools.rs:609-658):
  per-tool negative membership in BOTH tables for the five project tools, then
  wholesale `assert_eq!` of each table against its explicit expected list —
  moving any verb across the fence boundary in either direction goes red.
- AGENTS.md socket rule respected: the test asserts table membership, which is
  exactly the pure decision `dispatch` consults (src/mcp/tools.rs:327) — no
  `dispatch` call, so no real projects.yml write against the live server.
- No fence test for an out-of-scope channel is owed: Gate 3's decision is that
  no project verb needs channel fencing, and Gate 5 pins that decision.

## Gate 6 — `just check` / `cargo test mcp` verde
- EVIDENCE: pending — lead runs it (builder may not run cargo).

## Gate 7 — docs/next/CHANGELOG.md
- One bullet appended under the existing `## Unreleased` → `### Added`:
  docs/next/CHANGELOG.md:14. Root CHANGELOG.md untouched.
