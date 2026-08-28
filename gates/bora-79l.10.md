# bora-79l.10 — T6 · Sections montáveis em runtime

## Pass 6a (LANDED, commit 5b8d62cb)

Section-as-container: uma header por grupo de branch no topo, blocos membros
contíguos, cluster de diff somado. `p4a_project_view_capture_matches_alvo_line_by_line`
perdeu o `#[ignore]` e roda na suíte padrão.

## Pass 6b — o desenho (decidido pelo lead, 2026-08-28)

O gate (1) do bead ("resolver o problema da generic-row ANTES") é satisfeito
**por construção, não por trabalho**: nenhuma variante nova de
`WorkspaceListEntry` é criada. Toda section declarada renderiza pelas linhas
que já existem (`SectionRow`, `SectionHeader`, `SectionItem`, `PaneDotsRow`),
e `a_sixth_descriptor_renders_through_the_generic_rows_without_touching_the_registry`
(project_view.rs) já prova que um descriptor renderiza sem estar no REGISTRY.

O scout apontou `&'static SectionDescriptor` em `SectionHeader.kind` como "o
bloqueio mais difícil" para um registry em runtime. **Não é bloqueio: é o
eixo errado.** Os TIPOS de section são quatro e conhecidos em tempo de
compilação (BRANCH/COMANDO/CHECKS/LIVRE); o que nasce em runtime são as
INSTÂNCIAS (`ui::sidebar::sections::Section`), que já existem, já fazem
round-trip YAML e já são lidas pelo render (`project_view.rs:194` passa
`project.layout.as_deref()`; `section_model_flags` consome). Portanto:

- REGISTRY continua `const`, `SECTION_COUNT` continua fixo,
  `resolve_section_order` continua devolvendo array de tamanho conhecido —
  zero alocação nos laços multiplicativos (regra "Multiplicative performance
  paths" do AGENTS.md preservada).
- `SectionKind` → descriptor const: `Comando`→`COMMANDS`, `Checks`→`CHECKS`,
  `Livre`→ um `static LIVRE` NOVO e **fora** do REGISTRY (não é ordenável por
  `sections.order`, não tem wire name a defender).
- Nada de `Box::leak`, nada de arena, nada de campo novo em `AppState`.

Fonte da verdade = `projects.yml` (`Project.layout`), já lido por
`ProjectsStore` e já recarregado por `App::poll_projects_store`. Mutação =
`update_projects_file` (read-fresh → modifica → escreve), o poll devolve pro
render. Não existe cópia em memória, logo não existe drift a disciplinar.

`reconcile_section_layout` (restore.rs:948, hoje sem chamador) é chamado **no
site da mutação**, não a cada render nem em background: limpa sections órfãs
quando o usuário mexe, sem escrita de fundo.

## Contrato entre as fatias (fixado antes do fan-out)

```rust
// src/ui/sidebar/sections.rs
pub struct Section {
    pub id: String,
    pub kind: SectionKind,
    /// NOVO: nome de exibição da header. `None` = derivado (BRANCH usa a
    /// branch; COMANDO/CHECKS usam o label do descriptor).
    pub name: Option<String>,          // #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_on: bool,
    pub parts: SectionParts,           // { dots: bool, diff: bool }
    pub children: Vec<SectionChild>,
}

// src/app/sections.rs  (ARQUIVO NOVO — fatia A é a dona)
pub fn declared_section_for_checkout<'a>(
    projects: &'a crate::persist::projects::ProjectsStore,
    slug: &str,
    checkout_key: &str,
) -> Option<&'a Section>;

// src/api/schema.rs — verbos novos (aditivos: NÃO bumpar PROTOCOL_VERSION)
#[serde(rename = "project.section_create")]
ProjectSectionCreate(ProjectSectionCreateParams),   // { slug, kind, name: Option<String> }
#[serde(rename = "project.section_update")]
ProjectSectionUpdate(ProjectSectionUpdateParams),   // { slug, section_id: Option<String>,
                                                    //   checkout: Option<String>,
                                                    //   header_on/dots/diff: Option<bool> }
ResponseResult::ProjectSectionCreate { section_id: String }
```

`project.section_update` endereçado por `checkout` **materializa** a section
BRANCH daquele checkout quando o projeto ainda não declara nenhuma — é isso
que faz o toggle funcionar num `projects.yml` real sem `layout:` (todos hoje).

// src/worktree.rs
`generated_two_word_name(seed: u64) -> String` — "jolly-walrus", sem o prefixo
`worktree/` e sem o sufixo hex. `generated_branch_slug` passa a chamá-la (uma
lista de palavras só, não duas).

## Gates do bead

1. **generic-row ANTES** — satisfeito por construção: nenhuma variante nova.
   Prova: `just check` verde sem editar `entry_row_height`, os dois matches de
   `apply_hidden_filter`, `workspace_list_areas_for_entries` nem
   `render_workspace_list`. Um diff que toque esses sites reprova o gate.
2. **persist/restore testado** — teste que cria section via verbo, relê o
   arquivo do disco e prova que a section sobrevive; e teste que prova que
   `layout: None` (todo projeto real hoje) renderiza byte a byte como antes.
3. **`just check` verde + version bump.**
4. **iteração visual contra o instrumento de captura** (`ui::sidebar::capture`).

## Regra de teste vinculante para as três fatias

Todo teste novo precisa falhar se a lógica sair. Nada de teste que só
exercita o caminho feliz de serde. Especificamente proibido: teste que chama
`mcp::tools::dispatch`/socket sem `timeout_ms` (AGENTS.md — trava a suíte),
e teste que constrói `IsolatedStateDir` + `IsolatedConfigDir` separados
(usar `crate::config::IsolatedDirs`, senão deadlock).
