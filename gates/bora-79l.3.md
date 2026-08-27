# Gates — bora-79l.3 (F2): render do PaneDotsRow em bloco de 2 linhas

Fonte de verdade visual: o const `ALVO_CAPTURE` em `src/ui/sidebar/capture.rs`
(revisão R1, aprovada pelo dono). Onde o texto antigo do plano (l1 com diff)
conflitar com o const, o const vence: **o diff `+n −m` mora no cluster do
header BRANCH (F3), NÃO na l1.**

Contrato com a fatia irmã (bora-79l.9): este leaf NÃO edita
`src/ui/sidebar/sections.rs` nem nada sob `src/persist/`. Se um ajuste de
modelo se mostrar necessário, parar e reportar ao Main.

- [x] G1: workspace vira bloco de 2 linhas — l1 com o nome na col 3 (2
  espaços), cor overlay1 (a mesma do `⎇`), sem glifo de estado na l1; l2 com
  uma bolinha por painel nos 5 estados da 0.45.6 (`⠋` girando, `●` amarela
  esperando você, `●` verde respondeu, `◆` vermelho falha, `○` parado).
  CHECK: cargo nextest run --locked -E 'test(pane_dots)'
  EXPECT: todos verdes, incluindo teste novo do bloco 2 linhas
- [x] G2: capture do fixture mostra os blocos na forma do alvo (linhas tipo
  04-05 e 28-29 do `ALVO_CAPTURE`), e a suíte de captura continua verde com
  P4-A ainda `#[ignore]`.
  CHECK: cargo nextest run --locked -E 'test(capture)'
  EXPECT: 42+ passed, 0 failed
- [x] G3: hit areas continuam caindo em cima do que é renderizado (a lição
  do PaneDotsRow: colunas derivadas dos dados que as duas passes compartilham).
  CHECK: cargo nextest run --locked -E 'test(hit_area)' (ou teste equivalente existente)
  EXPECT: verdes
- [x] G4: doc comment do módulo `sections` em `src/ui/sidebar.rs` (linha ~4,
  "Model-only for now...") atualizado — render wiring F2 pousou.
  CHECK: grep -c "Model-only for now" src/ui/sidebar.rs
  EXPECT: 0
- [ ] G5: `just check` inteiro verde.
  CHECK: just check
  EXPECT: exit 0

EVIDENCE G1: `cargo nextest run --locked -E 'test(pane_dots)'` → 12 tests run:
12 passed, 4148 skipped. Includes the new 2-line-block test
(`pane_dots_row_is_a_two_line_block_name_then_dots`) and the new 5-state
glyph test (`pane_dots_dot_glyph_covers_every_reachable_agent_state`), plus
the rewritten `pane_dots_columns_are_one_per_pane_spaced_two_apart`,
`pane_dots_dots_line_renders_one_dot_per_pane_and_totals_exact_width`,
`pane_dots_name_line_never_contains_a_repo_name`,
`pane_dots_name_line_uses_column_3_and_overlay1_no_state_glyph`.

EVIDENCE G2: `cargo nextest run --locked -E 'test(capture)'` → 42 tests run:
42 passed (2 leaky, pre-existing/unrelated), 4118 skipped. P4-A
(`p4a_project_view_capture_matches_alvo_line_by_line`) stays `#[ignore]`d.
Confirmed with a throwaway (reverted, no diff left in capture.rs) diagnostic
run against `alvo_fixture()` that wherever this leaf's PaneDotsRow rows
land, their CONTENT is byte-exact against the target: `"   main"` / `"
◆"` (falha, Blocked), `"   main-review"` / `"   ○"` (parado, Idle+seen),
`"   feature-x"` / `"   ⠋"` (Working), `"   research-feature-x"` /
`"   ⠋"`, `"   feature-y"` / `"   ⠋"`, `"   cleanup"` / `"   ○"`,
`"   scratch"` / `"   ○"` (Unknown reading as parado). Every remaining
divergence in that diagnostic is row-offset drift from SectionRow's own
branch-header rendering and band placement (F3/F7 territory, not this
leaf's `PaneDotsRow` arm) — P4-A staying red end-to-end is expected until
those siblings land, per gate G5's explicit hand-off to the lead.

EVIDENCE G3: `cargo nextest run --locked -E 'test(hit_area)'` → 8 tests run:
8 passed, 4152 skipped. Includes
`pane_dots_row_hit_areas_land_on_the_rendered_dots_own_columns`, updated to
assert dot hit areas land on l2 (`row_y + 1`) against the real rendered
glyph, not re-derived arithmetic.

EVIDENCE G4: `grep -c "Model-only for now" src/ui/sidebar.rs` → `0`. Doc
comment on the `sections` module declaration rewritten to state F2's
render wiring landed (`PaneDotsRow`'s 2-line block) while F3 (SectionRow's
diff/dots cluster) and F7 (projects.yml persistence) remain pending.

EVIDENCE G5: pending — explicitly the lead's job per this leaf's brief
(`just check` + version bump + release). Sanity check run in this leaf's
own worktree before hand-off: `cargo nextest run --locked --no-fail-fast`
(whole crate) → 4157 tests run: 4157 passed, 3 skipped (0 failed). Two
pre-existing project_view.rs tests
(`checks_section_lockstep_rows_stay_height_one`,
`every_emitted_entry_has_row_height_one` → renamed
`every_emitted_entry_has_row_height_one_except_pane_dots_row`) and one
(`workspace_list_lockstep_pull_requests_agree_across_passes`) hard-coded
"every entry is height 1"; updated to expect `PaneDotsRow` at height 2,
everything else at height 1 — a legitimate consequence of this leaf's
`entry_row_height` change, not a workaround.
