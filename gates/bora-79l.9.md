# Gates — bora-79l.9 (F7): persistência projects.yml formato novo

Contrato com a fatia irmã (bora-79l.3): este leaf NÃO edita o caminho de
render em `src/ui/sidebar.rs` (pode LER; a edição de render é da F2). Dono
aqui: `src/ui/sidebar/sections.rs` (o modelo da F1 já tem o stub YAML),
`src/persist/` (`projects.rs`, `restore.rs`, `snapshot.rs`).

Formato novo = o shape do stub de `sections.rs` (kind lowercase, children
internamente tagged com `type`, `header_on`, `parts`), estendido com o que a
sessão precisa para restaurar: identidade da workspace (name + checkout) e
ids pinados.

- [x] G1: round-trip — salvar sections num projects.yml e reler devolve o
  mesmo modelo, com ids pinados preservados.
  CHECK: cargo nextest run --locked -E 'test(sections) or test(project_layout_round_trips)'
  EXPECT: /(\d+) tests run: \1 passed/
  EVIDENCE: 17 tests run: 17 passed, 4143 skipped — includes
  `sections_model_round_trip` (F1's tree, now with pinned `id`s) and the new
  `project_layout_round_trips_with_pinned_ids_through_projects_yaml`
  (persist::projects level: serialize → reparse the whole `ProjectsFile`,
  assert equality, assert the two section ids survive verbatim).
- [x] G2: fallback — um projects.yml no formato ATUAL (de hoje) continua
  carregando; nenhum teste existente do store quebra.
  CHECK: cargo nextest run --locked -E 'test(projects)'
  EXPECT: /(\d+) tests run: \1 passed/
  EVIDENCE: 30 tests run: 30 passed, 4130 skipped — includes the pre-existing
  `round_trips_the_design_example` (untouched `sections:` checks/commands/order
  shape, zero changes needed) plus the new
  `project_layout_is_absent_not_an_error_on_a_pre_layout_projects_file`,
  which parses the design-example YAML (no `layout:` key at all), asserts
  `layout` defaults to `None` with no error, asserts the old `sections:`
  field is untouched, and asserts a re-serialize omits `layout:` entirely.
- [x] G3: restauração na sessão — workspace com mesmo checkout reentra na
  section certa, na ordem salva, com o id pinado (caminho hoje:
  `src/persist/restore.rs`).
  CHECK: cargo nextest run --locked -E 'test(reconcile_section_layout)'
  EXPECT: /(\d+) tests run: \1 passed/
  EVIDENCE: 2 tests run: 2 passed, 4158 skipped — new
  `persist::restore::reconcile_section_layout` (pure function, matches
  `SectionChild::Workspace::checkout` against the session's live checkouts):
  `reconcile_section_layout_keeps_live_workspaces_in_saved_section_and_order`
  proves a stale checkout is dropped, a live one keeps its section by pinned
  `id` (not list position) and its saved order, its name refreshes to the
  live workspace's current name, and non-`Branch` sections pass through
  untouched; `reconcile_section_layout_of_an_empty_saved_layout_is_a_noop`
  covers the empty case. No production caller yet — `AppState` has nowhere
  to hold a per-project runtime layout until a later leaf wires one in
  (documented `#[allow(dead_code)]` on the function, not a stub).
- [x] G4: superfície de edição restrita — nada fora de sections.rs +
  src/persist/ + os pontos de wiring estritamente necessários.
  CHECK: git status --porcelain -- src/
  EVIDENCE: 11 files changed under src/. Owned (per assignment):
  `src/ui/sidebar/sections.rs` (added pinned `Section::id`),
  `src/persist/projects.rs` (added `Project::layout`, tests),
  `src/persist/restore.rs` (added `reconcile_section_layout`, tests).
  `src/persist/snapshot.rs`: read, no change needed. Every other file
  (`src/app/api/projects.rs`, `src/app/api/workspaces.rs`,
  `src/app/input/modal.rs`, `src/app/input/sidebar.rs`, `src/sandbox.rs`,
  `src/server/headless.rs`, `src/ui/sidebar/capture.rs`,
  `src/ui/sidebar/project_view.rs`) got exactly one mechanical line
  (`layout: None,`) added to an existing `Project { .. }` struct literal —
  `Project` derives no `Default`, so any new field breaks every literal
  construction site crate-wide; this is the strictly-necessary wiring the
  gate allows, not a scope expansion. `src/ui/sidebar.rs` (the forbidden
  render path) is untouched. Coordinated with F2Render (owns capture.rs and
  project_view.rs this wave) over hub before touching either file; F2Render
  acked and left both `layout: None,` additions to this leaf.
- [ ] G5: `just check` inteiro verde.
  CHECK: just check
  EXPECT: exit 0

ABANDON: G5 fica pro lead após o merge (fora do escopo deste leaf, per o
  Target da atribuição: "G5 (just check) fica pro lead").

Full-suite check beyond G1-G4's filters (not a named gate, extra evidence):
`cargo nextest run --locked` → 4157 tests run: 4157 passed, 3 skipped, 0
failed. Every existing test in the crate stays green with this leaf's
changes applied.
