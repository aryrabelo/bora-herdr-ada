# GATES — channel send: entrega imediata como default (agente/channel-inject-now)

Branch `agente/channel-inject-now`, worktree `~/Sites/bora-team/worktrees/bora/channel-inject-now`, base `origin/main` (`bc2c5914`).

## Contrato (decisões do dono, nao renegociar)

- Default do `channel send` = injeção imediata (mesmo default do `agent prompt`); `--when-idle` é o opt-in do comportamento antigo (`deferred` + `queue_position`).
- Entrega ao assento humano continua passiva (ceo-bora#33, binding) — `to_human` intocado.
- Damper de burst, resolução de nick e a fila `pending_agent_prompts` intocados.
- `CHANNEL_PROTOCOL_VERSION` 4→5 com reescrita do trecho que mentia ("deferred while the target is busy").
- Nome de canal com ou sem `#` resolve igual; canal inexistente = erro nomeado.
- `--timeout <MS>` NÃO foi adicionado: `channel send` vai direto pro App (sem o wait de conexão de `agent prompt` em `src/api/wait.rs::prompt_agent`), e o drain da fila usa o settle fixo — `when_idle_timeout_ms` não é honrado nesse caminho; seria flag morta.

## Gates

- [x] G1 — Fan-out default é imediato: membro ocupado (Working) recebe `delivered`
  CHECK: `cargo nextest run -E 'test(channel_send_to_working_member_injects_immediately_by_default)'`
  EXPECT: `1/1 passed`
  EVIDENCE: `Summary [ 0.068s] 4 tests run: 4 passed` (com os 3 outros gates novos). Teste novo: membro `Working` COM runtime recebe bytes `[<canal> seq=N from ...]` no buffer, receipt `delivered`, `pending_agent_prompts` vazio.

- [x] G2 — Opt-in preservado: `when_idle: Some(true)` mantém `deferred` + `queue_position`
  CHECK: `cargo nextest run -E 'test(channel_send_when_idle_defers_to_working_member)'`
  EXPECT: `1/1 passed`
  EVIDENCE: teste novo via `channel_with_two_agents` (worker Working) → status `deferred`, detail `queued (pos 1)` — mesmo recibo de hoje, `classify_delivery` intocado.

- [x] G3 — Nome com/sem `#` resolve igual; inexistente é erro nomeado
  CHECK: `cargo nextest run -E 'test(find_channel_workspace_matches_name_with_or_without_hash) or test(members_on_missing_channel_is_error)'`
  EXPECT: `2/2 passed`
  EVIDENCE: `find_channel_workspace` agora normaliza com `channels::normalize_channel_name` (reuso, sem segundo `trim_start_matches`); `members_on_missing_channel_is_error` já existia e continua verde.

- [x] G4 — Briefing diz a verdade (v5)
  CHECK: `cargo nextest run -E 'test(channel_protocol_briefing_teaches_immediate_delivery)'`
  EXPECT: `1/1 passed`
  EVIDENCE: texto novo ensina "arrives WHILE YOU ARE WORKING, like steering" + `--when-idle`; assert nega o texto velho ("deferred while the" não pode existir). Version 4→5 com doc comment da razão; re-brief garantido pelo gate `entry.version >= CHANNEL_PROTOCOL_VERSION` (já caracterizado por teste de resend existente). `clippy::assertions_on_constants` derrubou o assert de constante — removido, o bump fica provado pelo diff e pelos testes de resend.

- [x] G5 — Prova viva: `channel send` para pane ocupado devolve `delivered` e chega no pane
  CHECK: servidor descartável próprio (socket+config em `~/Sites/temp-files/20260904-bora-chan-proof/`, binário `target/debug/bora` da worktree, `bora --version` → `0.45.39 (v0.8.2[2c042bb2].bora-45.39)`)
  EXPECT: `"status": "delivered"` para o pane Working; texto no pane
  EVIDENCE: canal `#proof` com 2 membros agentes reais (gemini CLI detectado: `gemlab` idle, `gemwork` working — working atingido pela regra `esc_cancel_working` do manifest em tela real, sem chamada de modelo; p4 com `GEMINI_API_KEY=invalid-proof-key` pra isolar quota). Send default: `{"deliveries":[{"pane_id":"w1:p3","status":"delivered"},{"pane_id":"w1:p4","status":"delivered"}],"seq":1}` — **p4 estava `working` no momento do send**. Chegada visível: buffer do p4 renderiza `> [bora] channel protocol for #proof (v5):` (bytes digitados no pane); p3 idle→`working` após receber a mensagem (turno real iniciado pelo texto entregue). Repetição com p3+p4 working: ambos `delivered` (seq=5). Servidor derrubado (`hub stop`, exit 0, uptime 11m46s).

- [x] G6 — Prova viva do opt-in: `--when-idle` devolve `deferred` com `queue_position`
  CHECK: mesmo servidor; `bora channel send proof "..." --when-idle` com p3 e p4 `working`
  EXPECT: `"status": "deferred"`, `"detail": "queued (pos 1)"`
  EVIDENCE: `{"deliveries":[{"detail":"queued (pos 1)","pane_id":"w1:p3","status":"deferred"},{"detail":"queued (pos 1)","pane_id":"w1:p4","status":"deferred"}],"seq":4}` — comportamento de hoje preservado byte a byte no recibo.

- [x] G7 — CLI real: grafias idênticas; erro nomeado
  CHECK: `bora channel members proof` vs `bora channel members '#proof'` vs `bora channel members nope`
  EXPECT: saída idêntica nas duas grafias (exit 0); erro nomeado na inexistente
  EVIDENCE: ambas grafias → mesmas 4 linhas (`w1:p1/-/-/workspace`, `w1:p3/idle/gemlab/workspace`, `w1:p4/working/gemwork/workspace`, `w1:p2/-/-/workspace`), exit 0. `members nope`: stdout vazio, stderr `{"error":{"code":"channel_not_found","message":"channel #nope not found"},"id":"cli:channel:members"}`, exit 1.

- [x] G8 — Suíte verde
  CHECK: `just check`
  EXPECT: exit 0
  EVIDENCE: verde após regenerar artefato de schema (`HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current` — campo novo `when_idle` mudou o schema gerado; `docs/next/api/herdr-api.schema.json` atualizado). Fim: `Ran 143 tests in 5.895s / OK`, nextest completo, fmt limpo.

- [x] G9 — fmt + clippy alvo Linux (flags do CI, com touch)
  CHECK: `cargo fmt --check` e `LIBGHOSTTY_VT_PREBUILT=prebuilt/libghostty-vt-aarch64-macos.a cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings -A clippy::dbg_macro -A clippy::todo -A clippy::cognitive_complexity -A clippy::too_many_lines -A clippy::unwrap_used`
  EXPECT: ambos exit 0
  EVIDENCE: fmt `FMT_CLEAN`; clippy `Finished dev profile ... in 21.89s` após `touch src/app/api/channels.rs` (re-emissão real, não cache). `prebuilt/` copiado do checkout compartilhado (mesmo vendor commit, gitignored).

- [x] G10 — Mutação por resultado (4/4)
  CHECK: sed-muta + nextest do teste NOMEADO + sed-reverte + `grep -cF` no MESMO comando
  EXPECT: verde antes, FAIL depois, reversão provada
  EVIDENCE (todas contra `src/app/api/channels.rs`, cada uma partindo de suíte verde 76/76):
  - M1 (G1): `when_idle: params.when_idle,` → `when_idle: Some(true),` → `FAIL ... channel_send_to_working_member_injects_immediately_by_default` (`0 passed, 1 failed`). **Reverter largo demais**: o sed de reversão casou os 5 `when_idle: Some(true),` do arquivo e quebrou os outros 4 sites; reparado com `sed` endereçado por linha (1121/2399/2515/3030), verificado `grep -c` (`params.when_idle` = 1, `Some(true)` = 4) e suíte 76/76 de novo. Nenhum `git restore` usado.
  - M2 (G2): linha 654 → `when_idle: None,` → `FAIL ... channel_send_when_idle_defers_to_working_member`; reversão por linha; `grep -cF 'when_idle: params.when_idle,'` = 1.
  - M3 (G3): `let name = channels::normalize_channel_name(name);` → `let name = name.to_string();` → `FAIL ... find_channel_workspace_matches_name_with_or_without_hash`; reversão pelo mesmo padrão; `grep -cF` = 1.
  - M4 (G4): `passes --when-idle, and` → `passes --hold-idle, and` → `FAIL ... channel_protocol_briefing_teaches_immediate_delivery`; reversão; `grep -cF 'passes --when-idle, and'` = 1; suíte de canais 76/76 após as 4.

## Revisao do PR #20 (adendos pos-review, mesmos padroes)

- [x] G11 — P1 pos-review: briefing viaja no MESMO modo `when_idle` da mensagem; ordem briefing→mensagem; fila vazia no default; doc comment mentiroso reescrito
  CHECK: `cargo nextest run -E 'test(channel_protocol_and_message_inject_together_for_working_member)'` e a mutação M5: `sed -i '' 's\|^                when_idle,$\|                when_idle: Some(true),\|' src/app/api/channels.rs && cargo nextest run -E 'test(channel_protocol_and_message_inject_together_for_working_member)'; sed -i '' 's\|^                when_idle: Some(true),$\|                when_idle,\|' src/app/api/channels.rs; grep -c '^                when_idle,$' src/app/api/channels.rs`
  EXPECT: verde antes; **FAIL depois** da mutação; reversão provada (`grep` = 1) e reparo linha-a-linha dos literais de teste que a reversão larga alcançou (2407/2523/3095 → `Some(true)` de novo; medido: suíte de canais 76/76 após reparo)
  EVIDENCE: teste novo usa membro Working NÃO briefado (sem `skip_protocol`), runtime real no alvo: primeiro `try_recv` contém `channel protocol for #eng`, segundo contém `[#eng seq=` + texto, `pending_agent_prompts` do alvo vazio. Sob M5 o teste morre (briefing enfileirado, fila não-vazia). `send_channel_protocol` ganhou parâmetro `when_idle`; fan-out passa `params.when_idle`; `channel join` passa `Some(true)` (juntar-se nunca digita num turno rodando) — razões verdadeiras no doc comment, "queued rather than dropped" apagado (era falso desde o nascimento: nunca houve drop de prompt sem `when_idle`).

- [x] G12 — Revogação: o defeito de nome sem `#` NÃO EXISTIA (diagnóstico do dono era proxy de `jq`, exit 5 era do jq)
  CHECK: `grep -n 'normalize_channel_name' src/app/api/channels.rs` e `cargo nextest run -E 'test(find_channel_workspace)'`
  EXPECT: `find_channel_workspace` SEM normalização (voltou ao formato da base); teste `find_channel_workspace_matches_name_with_or_without_hash` REMOVIDO
  EVIDENCE: todos os 9 callers de produção já normalizavam antes (medido pelo revisor na base `bc2c5914`); a normalização adicionada era inalcançável em produção e custava uma alocação por lookup. Entrada do CHANGELOG: frase falsa sobre lookups de nome apagada; entrada agora só descreve a mudança de entrega.

- [x] G13 — Docs de usuário em `docs/next/website` refletindo o default novo
  CHECK: `grep -c 'when-idle' docs/next/website/src/content/docs/cli-reference.mdx docs/next/website/src/content/docs/agent-automation.mdx`
  EXPECT: ambos citam `--when-idle` com a semântica nova
  EVIDENCE: `agent-automation.mdx:78` agora diz "injected immediately into every member even mid-turn — like steering … `--when-idle` opts back into hold-until-idle deferral" (estava de cabeça pra baixo); `cli-reference.mdx` ganhou seção `## Channels` com o bloco de verbos incluindo `channel send … --when-idle` e o parágrafo de semântica de entrega.

Nota de contagem: o relatório anterior disse "10 de 10". Com a revogação do G3 (defeito inexistente) e o âmbito do G7 reduzido a observação CLI (as grafias sempre funcionaram; não é resultado deste PR), os gates de RESULTADO deste PR são G1, G2, G4, G5, G6, G8, G9, G10, G11, G12, G13 — todos com evidência colada aqui.

- [x] G14 — CI macos vermelho no PR: `open_twice_on_an_already_repaired_channel_does_not_duplicate_the_transcript_pane` sem `IsolatedDirs` (não-regressão: o teste já nascia sem isolamento no `main`; o conserto é nossa porque o PR está vermelho)
  CHECK: `grep -A2 'fn open_twice_on_an_already_repaired' src/app/api/channels.rs | grep -c IsolatedDirs` e auditoria `grep -c IsolatedDirs::new src/app/api/channels.rs`
  EXPECT: guard presente no teste; 69 guards no total (62 → 69)
  EVIDENCE: auditoria dos 14 testes de canal sem guard: 7 tocam estado em disco e receberam guard (`create_normalizes…`, `create_seeds…`, `create_succeeds_even…`, `create_gives…`, `open_repairs…`, `open_twice…`, `list_reports…` — este último via `create_channel`); os outros 7 são puros em memória (`open_on_unknown` erra antes de tocar disco, `from_human_cannot_be_claimed_over_the_wire`, `channel_protocol_briefing_teaches…`, `classify_delivery_maps_outcomes`, `member_addressable_name_falls_back…`, `leading_mention_nick_parses…`, `burst_active_counts…`) e ficaram como estão. Prova de sobrevivência ao ataque, medida: com o guarda REMOVIDO (mutação), suíte CHEIA 3× (4246 testes, `-j 16`: 20.5s/19.9s/20.4s, 4246/4246 passed) e suíte de canais 15× `-j 16` (76/76 em todas) — **nenhuma falha fabricada localmente**. LIMITE DECLARADO: a janela de corrida (mutex `IsolatedDirs` detido por outro teste no instante exato do `handle_channel_open`) não foi reproduzível nesta máquina; a evidência do conserto é o mecanismo nomeado pela regra binding do `AGENTS.md` (guarda process-wide; sem ele o teste resolve estado contra o diretório de OUTRO teste) + a vacina idêntica aos 62 irmãos + o CI do PR julgando no runner onde falhou.
  P3 de doc no mesmo commit: `cli-reference.mdx` `channel ask` agora lista `[--pane ID|--current]` (o verbo aceita; `parse_channel_ask_flags` em `cli.rs`).

## Nao-objetivos respeitados

- `to_human`/`notify_chat_to_human`: intocados (passivo, ceo-bora#33).
- Burst damper (`burst_active`, `record_channel_burst_send`): intocado.
- `member_addressable_name`: intocado.
- `pending_agent_prompts`: intocado (sirve `--when-idle` e `agent prompt --when-idle`).
- Briefing do protocolo (`send_channel_protocol`) segue com `when_idle: Some(true)` interno — a decisão de default imediato é para mensagens de canal, não para o bloco de ensino.
- `channel.ask` herda o default imediato pelo mesmo fan-out (pergunta enfileirada também era silêncio); sem flag própria.
