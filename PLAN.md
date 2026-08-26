# PLAN — B → A → C (plugin placement, PR sidebar rows, slot registry)

Owner decision 2026-08-24: do all three, in order, B prerequisite of C.
Research behind it: `.local/prd/plugin-extensibility.md`, `.local/prd/dynamic-sidebar-design.md`.

## Tree

```
L1  B → A → C
L2  B  activate PluginActionContext (bora-1e9)
    A  PR rows in the sidebar (bora-yw6)
    C  named-slot registry
L3  wave 1 (parallel, disjoint files):
      B1  menu placement mechanism + delete the dagr special case
      A1  PR check-status data
      A2  PR sidebar rows (three-pass lockstep)
    integration (lead-owned): mouse.rs hit-test wiring, AGENTS.md, version, changelog
    wave 2: C, scoped only after B1's mechanism exists
```

Depth 3. A and B are independent of each other; only C depends on B.

## Blocking decision, resolved before any code — capability model

The `bora-1e9` acceptance names this as blocking. Resolved NO: **a context-menu item that
fires a plugin action does not widen the threat model, so the menu path gets no special gate.**

Evidence, not judgement:

- `run_plugin_startup_hooks()` is called at `src/server/headless.rs:5221` and `:5333`, right after
  `print_ready_message` — i.e. at every server start, with **zero human interaction**.
- Startup hooks go through the same `start_plugin_command` as everything else, and that function
  sets `SOCKET_PATH_ENV_VAR` unconditionally (`src/app/api/plugins/runtime.rs:40-44`).
- Therefore installing an *enabled* plugin already grants arbitrary unattended code execution with
  the full unscoped wire API (`src/api/server.rs:1066` is a flat `match request.method` with no
  per-caller check).

A menu item requires an explicit human right-click and selection. That is strictly **more** gated
than `[[startup]]`, which ships today. Gating only the menu path while `[[startup]]` runs
unattended would be security theater — it would add friction exactly where the human is present
and none where they are absent.

Consequences, both mandatory:
1. Install-time trust is the real boundary, and that gets written down as a dated binding rule in
   AGENTS.md (integration gate GI2). Undocumented accepted risk is indistinguishable from an
   oversight.
2. The genuinely separable improvement — a per-plugin scoped RPC table instead of the whole wire
   protocol, the one thing worth stealing from deepseek-harness — is filed as its own bead. It is
   not a prerequisite of B and must not be smuggled into it.

## Cross-slice contracts — decided here, not negotiated by leaves

### C1. `OpenPr` gains a checks rollup

`src/workspace/git/open_prs.rs` currently has
`OpenPr { number, title, url, head_ref_name, is_draft, mergeable }`.

A1 adds exactly one field:

```rust
pub checks: PrChecksRollup,
```

```rust
/// Rollup of a PR's CI checks, derived from `gh pr list --json statusCheckRollup`.
/// Copy and allocation-free on purpose: this is read on the sidebar render path,
/// which is per-render x per-pane x per-client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrChecksRollup {
    /// No checks reported for the head commit.
    None,
    /// At least one check queued or running, none failed.
    Pending,
    /// Every completed check succeeded (or was neutral/skipped).
    Passing,
    /// At least one check failed, errored, timed out, or was cancelled.
    Failing,
}
```

`Failing` dominates `Pending` dominates `Passing` dominates `None`. A1 owns the precedence and the
GitHub-conclusion-string mapping; A2 owns only the glyph. **Not** a `Vec<CheckRun>`: a per-PR Vec on
the render path violates the multiplicative-performance rule, and the sidebar needs one glyph.

Widening the existing `gh pr list` call by one `--json` field is deliberate — correlating
`repo_open_prs` with the separate per-workspace `cached_check_status` cache would need two
independently-refreshed caches to agree, which they cannot be made to do.

### C2. The sidebar entry variant

A2 adds to `WorkspaceListEntry` (`src/ui/sidebar.rs`):

```rust
PrRow {
    number: u64,
    title: String,
    url: String,
    head_ref: String,
    is_draft: bool,
    checks: PrChecksRollup,
},
```

Height 1, like every existing variant. `entry_row_height` gets an arm; all three lockstep passes
get an arm. A2 does **not** touch `src/app/input/mouse.rs` — B1 owns that file this wave. A2's
deliverable ends at "the row renders and its geometry is in the areas walk"; the lead wires
hit-testing at integration, reusing the existing `ContextMenuKind::RepoPr`, which already carries
`{ws_idx, number, url, head_ref}` and already offers "Open in worktree".

### C3. Eligibility and placement of the PR band

Project-level, and it lists **only PRs with no local worktree** — the exact analogue of the dimmed
unopened-worktree row, and it prevents a PR appearing twice once opened. `project_view.rs`'s module
doc forbids project-level and worktree-level bands interleaving, so this is not a free choice.

### C4. File ownership this wave — strictly disjoint

| leaf | owns |
|---|---|
| B1 | `src/app/state.rs`, `src/app/input/modal.rs`, `src/app/api/plugins/mod.rs`, `src/app/input/mouse.rs`, and every remaining dagr special-case site |
| A1 | `src/workspace/git/open_prs.rs` |
| A2 | `src/ui/sidebar.rs`, `src/ui/sidebar/project_view.rs` |
| lead | `src/app/input/mouse.rs` hit-test for the new row (after B1 lands), `AGENTS.md`, `Cargo.toml`, `docs/next/CHANGELOG.md` |

No leaf runs `cargo fmt`, `just check`, `just lint`, or any commit/branch/push. The lead runs gates
once after merge.

## Status log (append only)

- 2026-08-24 — capability decision resolved with evidence; contracts C1–C4 fixed; wave 1 briefed.

- 2026-08-24 — owner named the quality bar: Solo's sidebar (soloterm.com). Read the real artifacts
  rather than the copy; 11 concrete elements extracted in `.local/prd/sidebar-gauntlet.md`. Solo
  cannot be the blind comparison artifact (native GUI vs TUI reveals the mapping, so the verdict
  would be medium not quality) — it is the criteria source, and the theirs-side is our own
  pre-change rendering via `git show`. Gauntlet is wave 3: A2 owns the sidebar files this wave, and
  the capture harness lives in them.
- 2026-08-24 — owner design north: arbitrary grouping, everything is a workspace, a workspace is a
  container of attachments (its PR, its state, a todolist, a checking command). Solo's hierarchy is
  fixed; ours is chosen. This reorders wave 3: chrome polish is cheap and secondary, and option C
  (the attachment/slot registry) is PROMOTED from "someday" to the phase that makes the product
  claim true, filed as its own epic. Its two obstacles (no-alloc render path, three-pass lockstep)
  are unchanged by the promotion.
- 2026-08-24 — cross-cutting bug found by leaf A1 and fixed by the lead at the root: `checks_rollup`
  had two silent fallthroughs to `Passing` — a `COMPLETED` run with `conclusion: None` (which GitHub
  really emits) and a `COMPLETED` run with any conclusion string added to the API after the code was
  written. Both displayed as a green tick. The module doc promised its consumers "can never drift
  apart" while each inferred non-failing states independently; they drifted the moment a fourth
  consumer appeared. Now one owner (`run_state`) with `check_run_state`/`reduce_run_states` exposed,
  and the counts/rollup invariant asserted over the whole status x conclusion cross product instead
  of promised in prose.
- 2026-08-24 — six `OpenPr {` literal sites broke on the new field; five patched by the lead,
  `mouse.rs` deferred to B1's release. Process lesson, per gauntlet-orchestration: enumerate unowned
  files BEFORE dispatching. A leaf caught this for me, which is luck, not method.- 2026-08-26 — bora-c1h closed at 13/14 gates. The two defects that mattered were found by
  reading the approved mock's CSS, not by looking at it and not by the blind critic: the state
  cluster was inline where `.sec .st { float: right }` pins it to the row edge, and the behind
  arrow was red where `.behind` is yellow and red is `.fail`'s alone. Both survived a green
  `just check` and a won critic round, because a screenshot cannot show either at sidebar width
  and the critic judged a lossy text extract. Promoted to a dated binding rule in AGENTS.md.
  The right-edge fix is mutation-verified (deleting the pad block reddens exactly its test).
  G13 (live handoff) is handed to the operator: this agent runs inside a pane of the server the
  handoff replaces.
- 2026-08-26 — P0 do dono: dois projetos no mesmo diretório colapsavam num só grupo. Causa não era
  render, era modelo: o projeto de um workspace era DERIVADO do path, e `claimed` dava a posse ao
  primeiro slug em ordem alfabética. Com `worktrees: all` em todo member (o yml real do dono), um
  member reivindica todo worktree do repo. Corrigido em 3 slices paralelas com binding explícito
  vencendo a derivação, fallback em binding órfão, e desempate por especificidade do member. Promovido
  a regra datada em AGENTS.md. Dois achados que valem mais que o patch: o site de literal em
  restore.rs (sem ele o binding evapora no restart — pior forma do bug) foi achado por um subagente
  DEPOIS de o lead concluir "zero sites" de um grep de 239 hits ruidosos; e `visual_group` foi
  auditado e NÃO é o mesmo conceito, então não houve consolidação errada.
- 2026-08-26 — gauntlet da sidebar: barra reconstruída como `bar/mock-capture.txt` (mock no MESMO
  formato do capture harness, 56 col) + `bar/gap-table.md` + `PROMPT.md`. Medição que reenquadra tudo:
  o HTML do mock contém UM glifo NF (e725, 23x); os outros dez que a legenda documenta estão ausentes
  byte-a-byte, então `.dirty/.ok/.run/.fail` são spans vazios. A forma aprovada é mais SILENCIOSA que
  a implementação — o problema é ruído a mais, não detalhe a menos.
