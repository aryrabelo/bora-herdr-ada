# GATES — transporte de bora channel (escopo B′), modo acelerado

Servidor Dolt fora do caminho crítico: bd é registro, não gate. Merge direto em
`main`, checagens locais completas, CI vigiado em background por subagente.

## bora-3k2.1 — eco do broadcast · FECHADO

- [x] G1 filtro do remetente vive em `channel_agent_member_pane_ids`, nenhum chamador
  EVIDENCE: `grep -c 'member.public_id != sender_pane'` = 1
- [x] G2 teste de regressão passa
  EVIDENCE: `test result: ok. 1 passed` — `broadcast_never_delivers_to_its_own_sender`
- [x] G3 teste fica VERMELHO sem o filtro
  EVIDENCE: mutante (linha do filtro removida) → FAILED, `deliveries` trouxe 3 panes
  incluindo o remetente `w2:p1`
- [x] G4 `just check` verde
  EVIDENCE: `3804 tests run: 3804 passed` (+1 vs. 3803 da baseline)
- [x] G5 changelog em docs/next/CHANGELOG.md
- [x] G6 commit no main, pushado
  EVIDENCE: `51f63571`

## bora-3k2.2 — seq no texto injetado · FECHADO

- [x] H1 prefixo entregue carrega o seq: `[#canal seq=N from <pane> <nick>]`
- [x] H2 `CHANNEL_PROTOCOL_VERSION` → 3, porque a forma injetada mudou
- [x] H3 teste prova que o seq visível FUNCIONA como cursor
  EVIDENCE: parseia o seq dos bytes entregues e alimenta `channels::read_since`, a
  mesma chamada de store que `channel.wait` / `tail --after` faz
- [x] H4 teste fica VERMELHO sem o seq
  EVIDENCE: `no parsable seq= in injected prefix: [#eng from w2:p1 first] first message`
- [x] H5 os dois testes que fixavam o prefixo antigo foram atualizados, não afrouxados
  EVIDENCE: exigem `[#eng seq=1 from ` / `[#eng seq=2 from `; falharam de verdade antes
- [x] H6 `just check` verde: `3805 tests run: 3805 passed`
- [x] H7 changelog + `agent-automation.mdx` atualizados
- [x] H8 commit no main: `2301ad31`

## INCIDENTE — CI vermelho desde antes deste trabalho · FECHADO

- [x] X1 causa identificada, e não era minha
  EVIDENCE: `168bbcd0` (só `.beads/issues.jsonl`) já falhava. `9a2db191` deixou
  `std::sync::{Mutex, OnceLock}` sem uso em `src/platform/linux.rs:789`; clippy roda
  com `-D warnings` e o módulo é `#[cfg(target_os = "linux")]`, invisível no macOS.
- [x] X2 conserto pushado: `ec277eb8`
- [x] X3 a fresta que deixou passar foi fechada: `ec277eb8` → `80751f94`
  EVIDENCE: o aviso do `just lint` só grepava `#![cfg(not(target_os = "macos"))]`,
  então os módulos de plataforma (excluídos por `#[cfg]` EXTERNO no `mod`) nunca
  apareciam. Agora nomeia os quatro arquivos — verificado rodando `just lint`.
- [x] X4 regra do AGENTS.md reafirmada com o segundo caso e com o motivo de não dar
  pra verificar cross-compilando aqui (libghostty-vt exige zig 0.15.2, mise resolve 0.16.0)
- [x] X5 CI verde confirmado
  EVIDENCE: run 32597639602 em `73ec2850` — `check (ubuntu-latest)` success (5m40s),
  `check (macos-latest)` success, `conventional-commits` success

## bora-3k2.3 (P5) — namespace de nick e assento humano · FECHADO

- [x] I1 o blob define as TRÊS formas de nick
- [x] I2 o blob nomeia o humano de verdade
  EVIDENCE: sufixo interpolado por instalação, `The human on this channel is @<nome>`
- [x] I3 teste fica vermelho se qualquer forma sair do blob
  EVIDENCE: 4 mutantes, 4 pegos. E o primeiro corte do teste NÃO pegava: um
  `contains("w78p1")` passava por menção incidental mais adiante no blob mesmo com a
  definição apagada. Reescrito para afirmar a LINHA que define cada forma.
- [x] I4 `just check` verde: `3806 tests run: 3806 passed`
- [x] I5 commit no main: `73ec2850`

## bora-3k2.4 (P4) — persistir pending_agent_prompts · FECHADO

- [x] K1 store durável, atômico, com leitura tolerante a arquivo corrompido
  EVIDENCE: `src/persist/pending_prompts.rs`; 3 testes próprios verdes (round-trip em
  ordem, ausente/corrompido lê vazio, escrita vazia remove o arquivo)
- [x] K2 fila carregada no boot e persistida em toda mutação
  EVIDENCE: `App::load_pending_agent_prompts` / `persist_pending_agent_prompts`;
  chamadas em enqueue, drain (ANTES do replay) e fail. `queue_id` avança além do
  maior id restaurado.
- [x] K3 achado durante o trabalho: os testes passaram a escrever no state dir REAL
  EVIDENCE: `~/.local/state/bora-dev/pending-prompts.json` apareceu com dados de teste
  (`p_target`, `msg-0`). Removido. Primeira tentativa de conserto (guarda por teste,
  17 testes) criou uma CORRIDA: `XDG_STATE_HOME` é global ao processo, e
  `startup_hooks_run_once_with_plugin_environment` passou a falhar na suíte completa
  enquanto passava sozinho. Guardas revertidas.
- [x] K4 conserto no nível certo: `state_dir()` sob `#[cfg(test)]` cai num temp por
  processo quando `XDG_STATE_HOME` não está setado
  EVIDENCE: `src/config/io.rs`, `test_default_state_dir()`
- [x] K5 helper órfão removido antes de virar quebra-CI
  EVIDENCE: `IsolatedStateDir` em `app::api::test_support` ficou sem uso depois da
  reversão de K3; clippy roda com `-D warnings` no CI, então `dead_code` quebraria a
  build. Removido, e a referência a ele no doc de `test_default_state_dir` repontada
  para as duas cópias que continuam em uso (`app::api::channels`, `app::input::chat`).
- [x] K6 suíte completa verde com K4 aplicado
  EVIDENCE: `3831 tests run: 3831 passed, 1 skipped` (baseline da sessão anterior: 3809).
  K4 precisou de dois consertos de clippy que nunca foram rodados: `needless_return`
  no `state_dir()` e `dead_code` em `platform_state_dir`, que sob `#[cfg(test)]` fica
  sem chamador — agora `#[cfg(all(..., not(test)))]` nas duas variantes.
- [x] K7 teste de aceite do bead: enfileira, derruba o servidor, sobe, mensagem entrega
  EVIDENCE: `app::api::agents::tests::deferred_prompt_survives_a_server_restart_and_delivers`.
  Primeira versão falhou por um motivo que valia descobrir: o contador de
  `generate_workspace_id` é global ao processo, então o segundo `App` cunha `w2:p1` e a
  fila restaurada — corretamente chaveada pelo id que o remetente recebeu — não tinha
  pane pra drenar. Restore real reatribui os ids salvos (`reserve_workspace_ids`), e o
  teste passou a fazer o mesmo. O disco estava certo desde o começo.
- [x] K8 dois mutantes provam que o teste não é cego
  EVIDENCE: (1) `load_pending_agent_prompts` fora do construtor → FAILED em "must come
  back from disk on boot"; (2) `persist_pending_agent_prompts` fora do drain → FAILED.
  Harness em `~/Sites/temp-files/mut/harness.py`.
- [x] K9 as três cópias do `IsolatedStateDir` viraram uma
  EVIDENCE: o comentário em `pending_prompts` pedia extração "quando aparecer a quarta",
  e o teste de K7 era a quarta. Agora `config::io::IsolatedStateDir`, usada por
  `app::api::channels`, `app::input::chat`, `persist::pending_prompts` e o teste novo.
  `persist::projects` mantém a sua própria porque isola `XDG_CONFIG_HOME`, variável
  diferente.

## bora-7c5.1 — colapsar mensagens longas · FECHADO

Gates C1–C7 preenchidos pelo builder com file:line (clamp `MAX_MESSAGE_LINES = 8`,
contagem escondida derivada do wrap e não de chars, expansão de uma mensagem só,
`request_full_repaint` no toggle, render puro com o estado em `ChatViewState`, marcador
em inglês pra casar com as strings vizinhas). O lead fechou o que faltava:

- [x] C8 `just check` verde: `3831 tests run: 3831 passed`
- [x] C9 mutante confirma que o teste não é cego
  EVIDENCE: `let shown = total;` (clamp fora) → `long_message_clamps` FAILED.

## bora-rlu.1 — truncar labels de header e branch · FECHADO

- [x] R1 `ProjectHeaderBranch.label` e `BranchHeader.label` passam por `truncate_end`
  EVIDENCE: `src/ui/sidebar.rs:2105` e `:2258`, orçamento medido com `display_width`
  espelhando a aritmética do braço Workspace.
- [x] R2 testes fixam a saída em 20/30/40 colunas com o `…` literal
- [x] R3 dois mutantes, um por label, os dois pegos
  EVIDENCE: label cru em `ProjectHeaderBranch` → `project_header_branch_label_truncates`
  FAILED; label cru em `BranchHeader` → `branch_header_label_truncates` FAILED.
- [x] R4 `just check` verde

## bora-jp0 — alargar o allowlist do MCP · FECHADO, com um gap achado no aceite

- [x] P1 os dez verbos existem de verdade no socket
  EVIDENCE: o lead conferiu cada um contra `api_method_name` (`src/api/server.rs`) — 10/10
  com braço próprio — e leu os dez params structs: só `AgentStartParams` tem `name`, e é
  nome de agente; nenhum tem `from_pane`. A dedução do builder sobre as tabelas de escopo
  se sustenta.
- [x] P2 REPROVOU na terceira cláusula do aceite ("--channels fencing still holds")
  EVIDENCE: `events.wait` aceita `match_event: {event: "channel_message", channel: X}` e
  `EventData::ChannelMessage` carrega `text`/`from_name` — um servidor cercado por
  `--channels eng` lia o tráfego de qualquer canal, o que tornava o fence do
  `channel_history` decorativo. Re-despachado com o gap nomeado.
- [x] P3 fence aninhado, falhando fechado
  EVIDENCE: `fence_events_wait_match_event`; mutante removendo a chamada do `dispatch`
  → `dispatch_rejects_events_wait` FAILED. Mutante removendo `("events.wait", None)` do
  allowlist → `generates_a_tool_per_present_allowlisted_variant` FAILED.
- [x] P4 dois testes entregues iam ao SOCKET e travavam a suíte
  EVIDENCE: `events.wait` é long poll; os testes de "passa o fence" chamavam `dispatch`
  inteiro, então numa máquina com servidor bora vivo esperavam para sempre — 15 min por
  teste, `just check` de 35s virou timeout de 40 min. Reescritos contra a decisão pura.
  Pior: `agent_start_name_param_is_not_a_fenced_channel` chamava `dispatch("agent_start")`,
  que num host com servidor vivo INICIA um agente de verdade; trocado por asserção na
  tabela que decide. E o teste de rejeição ganhou `timeout_ms: 1`, senão a ausência do
  fence vira stall de CI em vez de teste vermelho.
- [x] P5 `just check` verde

## bora-e9i.1 — projects.yml schema, parser, watcher · FECHADO

- [x] E1 schema do design, round-trip, com os dois defaults derivados
- [x] E2 resolver dir → (repo_identity, checkout_key, subdir); não-git resolve como
  `Unresolved`, nunca panic
- [x] E3 erro de parse mantém o último valor bom
- [x] E4 reload por mtime+len, um `stat` no caminho sem mudança — sem dependência de
  filesystem watch, seguindo o idioma de `detect::manifest`
- [x] E5 decisões de dependência tomadas pelo lead, não pelo builder: `serde_yaml_ng`
  (zero crates YAML na árvore antes disso, `serde_yaml` está arquivado) e poll de mtime
  em vez de `notify`
- [x] E6 o arquivo NÃO COMPILAVA quando entregue
  EVIDENCE: `const EXAMPLE_YAML: &str = r#"..."#` contendo `"#cnb"` — a sequência `"#`
  fecha um literal `r#"`. Virou `r##`. O builder tinha "verificado relendo os hunks",
  que é exatamente o que releitura não pega.
- [x] E7 três mutantes, três pegos
  EVIDENCE: erro de parse sobrescrevendo o último valor bom → `malformed` FAILED;
  short-circuit de arquivo inalterado removido → `reload_if_changed` FAILED; default de
  canal sem derivar do slug → `round_trips` FAILED.
- [x] E8 `just check` verde

## Registro

- [x] J1 épico da trilha visual registrado
  EVIDENCE: `bora-7c5` + `.1`(P1 colapso) `.2` `.3` `.4`, arestas `.1→.2→.3→.4`
- [x] J2 vigias de CI rodaram e reportaram
- [x] J3 fechar 3k2.1, 3k2.2 e 3k2.3 no bd
  EVIDENCE: fechados. O servidor Dolt deu erro intermitente ("may not be running") e
  exigiu retry por bead — 3k2.1 na primeira tentativa, .2 e .3 na segunda.
- [x] J4 bump de versão, porque `Cargo.toml` mudou (dependência nova)
  EVIDENCE: `0.33.0` → `0.34.0`, `Cargo.lock` atualizado no mesmo commit.
- [x] J5 entrada de changelog do jp0 corrigida: afirmava que o fencing "behaves exactly
  as before", o que deixou de ser verdade quando o fence aninhado entrou.
