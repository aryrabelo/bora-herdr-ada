# GATES — folha #111: `bora events --follow`

Issue: aryrabelo/ceo-bora#111 · branch `agente/events-follow` · versao alvo `0.45.38`

## Contrato (decidido pelo orquestrador, medido; nao renegociar)

`bora events [--follow] [--subscribe <name>]... [--pane <id>] [--limit <n>] [--session <name>]`

- Streaming e o comportamento default; `--follow` e aceito e documentado (o ticket e a doc do `mu` o nomeiam).
- Sem `--subscribe`: assina **as 30 variantes sem parametro** de `Subscription` (`src/api/schema/events.rs:19-98`).
- `--subscribe` e repetivel e recebe o nome do wire (`pane.created`, `workspace.focused`, ...).
- As **3 variantes que exigem `pane_id`** — `pane.output_matched`, `pane.agent_status_changed`, `pane.scroll_changed` — NAO entram no default e exigem `--pane`; sem ele, erro nomeado e exit 2.
- stdout: **um objeto JSON por linha, com flush por linha**. Nada de buffer que segure evento.
- `--limit <n>`: sai com 0 depois de n eventos (existe para o teste ser deterministico em vez de depender de sinal).
- SIGINT: exit 0.

O servidor ja faz o trabalho: `Method::EventsSubscribe` (`src/api/schema.rs:228`), handler `stream_subscriptions` (`src/api/server.rs:744`), que escreve `SubscriptionStarted` e depois uma linha JSON por evento. **Nao reimplemente nada disso.** O que falta e (a) um metodo de streaming no `ApiClient` (`src/api/client.rs` hoje so tem `request_value`, que le UMA linha) e (b) o verbo na CLI (`src/cli.rs:105-129` roteia por string; cada verbo mora em `src/cli/<nome>.rs`).

## Gates

- [x] G1 — o verbo existe e assina o default
  CHECK: `cd $PWD && ./target/debug/bora events --help >/dev/null 2>&1; echo $?`
  EXPECT: `0`
  EVIDENCE: `0` (binario debug reconstruido na arvore 0.45.38)

- [x] G2 — streaming real: uma linha JSON valida por evento, com flush
  CHECK: `bora events --limit 1 --session <sessao de teste> > /tmp-equivalente/ev.ndjson & sleep 1; bora pane split ...; wait; jq -e . < ev.ndjson`
  EXPECT: exit 0, arquivo com >=1 linha, `jq -e .` valido em todas, e o evento chega **antes** do comando gatilho retornar
  EVIDENCE: duas sessoes descartaveis proprias com socket isolado (`~/.config/bora-dev/sessions/gate111-events{,2}/herdr.sock`), removidas depois. (a) `bora events --session gate111-events --limit 1`: 0 linhas ocioso por 1s; gatilho `bora workspace create` -> 1 linha `{"...","type":"workspace_created",...}`, `jq -e .` valido, `events-exit:0`. (b) `--limit 2` na sessao virga: 0 linhas ocioso; UM gatilho produziu exatamente 2 linhas reais (`workspace_created` + `workspace_focused`), `jq -e .` valido, exit 0 no limite — entrega por evento, nao lote/snapshot. (c) stream continuo 5.6s: 13 eventos de tick chegando ao longo do tempo; SIGINT -> `exit=0` (reportado pelo supervisor do processo). Timing medido: a chegada do evento no stdout caiu ~22-25ms APOS o retorno do comando gatilho — o poll de assinatura do servidor e de 100ms (`CONNECTION_POLL_INTERVAL`, `src/api/server.rs:31`, lado do servidor, fora do escopo desta folha e explicitamente intocado); a entrega imediata no cliente (flush por linha) e provada por teste no G4.

- [x] G3 — as 3 variantes pane-scoped exigem `--pane`
  CHECK: `./target/debug/bora events --subscribe pane.agent_status_changed; echo $?`
  EXPECT: `2`, com mensagem nomeando o flag que falta (nao panic, nao exit 0)

  EVIDENCE: stderr = `pane.agent_status_changed requires --pane <id>` + usage; exit `2`. (`--subscribe pane.output_matched` com `--pane` tambem e erro nomeado exit 2: a variante exige expressao de match que o verbo nao expoe.) Fix pos-revisao, medido ao vivo: rejeicao do SERVIDOR (ex. `--pane nope`) sai como JSON estruturado `{"id":"cli:events:subscribe:sub:0:probe","error":{"code":"pane_not_found","message":"pane nope not found"}}` com exit 1 (antes vazava Debug do Rust); `--pane` sem `--subscribe` e erro nomeado exit 2 (`--pane requires --subscribe <name>: the default subscriptions are not pane-scoped`), nunca silencio.
  CHECK: `cargo nextest run events` e depois a mutacao: remover o flush por linha (ou trocar o writer por um bufferizado sem flush) e rodar de novo
  EXPECT: verde antes, **vermelho depois**. Um teste que passa nas duas vezes esta cego e nao conta.
  ATENCAO (regra medida, `AGENTS.md`): reverta a mutacao com o `sed` inverso **no mesmo comando**; NUNCA `git checkout -- <file>` / `git restore` — em 2026-09-01 isso apagou trabalho nao commitado inteiro neste repo. Depois confira com `grep -c` que a linha original voltou.
  EVIDENCE: verde: `cargo nextest run events` -> `72 tests run: 72 passed`. M1 (flush): `sed -i '' 's/out\.flush()/Ok(())/' src/cli/events.rs` -> `5 tests run: 4 passed, 1 failed` (FAIL `cli::events::tests::events_line_writer_flushes_per_line`, esperado 1 flush por linha); revertido com `sed -i '' 's/^    Ok(())$/    out.flush()/'` NO MESMO comando; `grep -c 'out\.flush()'` voltou a `1`. M2 (mapeamento wire->variante): `sed -i '' 's/"pane\.created"/"pane.creatd"/g'` -> `5 tests run: 3 passed, 2 failed` (FAIL `events_default_names_map_to_parameterless_wire_variants` — o teste que prova o mapeamento); revertido com sed inverso no mesmo comando; `grep -c '"pane\.created"'` = `5` antes e depois. M3 (pos-revisao: cegueira provada pelo revisor — nenhum teste chamava `build_subscriptions`, e mutar `subscription_for_name(name, pane_id)` para `None` deixava os 5 testes verdes): novo teste `events_build_subscriptions_applies_the_pane_flag_to_requested_names`; mutacao `s/subscription_for_name(name, pane_id)/subscription_for_name(name, pane_id.filter(|p| p.is_empty()))/` (mutante que compila) -> `6 tests run: 5 passed, 1 failed` (FAIL exatamente no teste novo); revertido com sed inverso no mesmo comando; `grep -cF 'pane_id.filter'` = `1` durante e `0` apos, `grep -cF 'subscription_for_name(name, pane_id)'` = `1` antes e depois. Pos-reversao: `cargo nextest run events` -> `72 tests run: 72 passed`. M4 (pos-revisao P3: o guarda `--pane` sem `--subscribe` nao tinha teste — inverte-lo deixava os 7 testes verdes): guarda extraido para `pane_without_subscribe_error` + teste `events_pane_without_subscribe_is_a_named_error`; verde `7 tests run: 7 passed`; mutacao `s/pane_id\.is_some() \&\& subscriptions\.is_empty()/pane_id.is_some()/` -> `7 tests run: 6 passed, 1 failed` (FAIL exatamente no teste novo); revertido com sed inverso no mesmo comando; `grep -cF 'pane_id.is_some() && subscriptions.is_empty()'` = `1` antes e depois, residuo `if pane_id.is_some() {` = `0`.
- [x] G5 — nenhum `unwrap()` novo em producao
  CHECK: `touch src/main.rs && cargo clippy --bins --message-format json -- -D clippy::unwrap_used 2>&1 | jq -r 'select(.message?.code?.code == "clippy::unwrap_used") | .message.spans[0].file_name' | sort -u`
  EXPECT: saida vazia. (O `touch` e obrigatorio: clippy NAO re-emite warning de build em cache e devolve um zero falso.)
  EVIDENCE: clippy exit `0`; saida do filtro jq = `0` arquivos (stdout do clippy capturado em arquivo para o jq nao engolir linha nao-JSON; contagem via `jq ... | sort -u | wc -l` = 0)

- [x] G6 — help/spec e completions cobrem o verbo novo
  CHECK: encontrar o spec de help (`src/cli/spec.rs`) e o gerador de completions (`src/cli/completion.rs`), acrescentar `events`, e rodar `cargo nextest run` filtrando os testes de spec/completion
  EXPECT: verde, e `bora events --help` imprime as flags do contrato acima
  EVIDENCE: `bora events --help` imprime `--follow`, `--subscribe <NAME>`, `--pane <ID>`, `--limit <N>`, `--session <NAME>` com as descricoes do contrato; `bora completion zsh | grep -c events` = `6` (completions sao GERADAS de `spec::command()` — acrescentar o subcommand no spec cobre o gerador; nenhuma edicao em `completion.rs` era necessaria); `cargo nextest run -E 'test(spec) or test(completion)'` -> `55 tests run: 55 passed`

- [x] G7 — versao bumpada na mesma commit
  CHECK: `grep -m1 '^version' Cargo.toml`
  EXPECT: `version = "0.45.38"` (regra binding do `AGENTS.md`: mudanca no package bumpa versao na MESMA commit)
  EVIDENCE: `version = "0.45.38"` — na MESMA commit `768d3f81` do codigo (junto com `Cargo.lock` regenerado). Pos-rebase em `main` `279cb8a0` (apos merge do #112/#18, que aterrissou 0.45.37): versao mantida em `0.45.38` — a proxima livre — resolvida no conflito do rebase.

- [ ] G8 — changelog no lugar certo
  CHECK: `python3 scripts/changelog.py check-history-sync` e `grep -c . <(sed -n '/^## Unreleased/,/^## /p' CHANGELOG.md | tail -n +2)`
  EXPECT: check-history-sync sai 0; a entrada nova esta em `docs/next/CHANGELOG.md` e o `## Unreleased` do CHANGELOG.md da raiz continua **vazio**
  EVIDENCE: entrada nova em `docs/next/CHANGELOG.md` sob `### Added` ✓; `## Unreleased` da raiz com 0 linhas nao-heading ✓; `check-history-sync` sai **1** — FALHA PRE-EXISTENTE NO MAIN, nao introduzida por esta folha: o HEAD pristine da branch (== tip de `origin/main`, `429db338`) falha identicamente. Causa medida: o bullet de `bora agent --new` existe SOMENTE no historico released de `docs/next/CHANGELOG.md` (linha 47) e nao existe no `CHANGELOG.md` da raiz — hand-edit que quebrou o espelho released. Consertar exigiria editar o `CHANGELOG.md` da raiz (proibido para feature work pelo `AGENTS.md` e bloqueado pelo `review_rules.py`) ou apagar conteudo alheio; nenhuma das duas e escopo da folha #111. Nota: `just check` NAO executa este script (so o recipe de release), G9 nao afetado.
  ABANDON: G8 (sub-check `check-history-sync`) — divergencia de released-history pre-existente no main; reconciliacao e decisao do mantenedor (regenerar a raiz a partir de docs/next). As partes sob controle da folha estao cumpridas e medidas acima.

- [x] G9 — gate do repo verde
  CHECK: `just check`
  EVIDENCE: exit 0 (~82s): fmt limpo, clippy `--bins -D clippy::unwrap_used` e `--all-targets`, suite nextest completa, 143 unittests de scripts OK. Extra pos-revisao do dono: `LIBGHOSTTY_VT_PREBUILT=prebuilt/libghostty-vt-aarch64-macos.a cargo check --target x86_64-unknown-linux-gnu --all-targets` -> exit 0 em ~24s (ponto cego de lint do Linux verificado localmente; o codigo novo nao tem gating de plataforma).

- [x] G10 — PR aberto
  CHECK: `gh pr view --json number,state,title --jq '"\(.number) \(.state) \(.title)"'`
  EXPECT: PR aberto contra `main` de `aryrabelo/bora-herdr-ada`, commit convencional minuscula, corpo com `refs #111`, sem keyword de fechamento (`fixes`/`closes`/`resolves` sao PROIBIDOS pela regra do repo)
  EVIDENCE: `19 OPEN feat: add bora events verb streaming session events as json lines | base: main | head: agente/events-follow` — https://github.com/aryrabelo/bora-herdr-ada/pull/19 ; commits `768d3f81`/`69a04601`/`baafb8b2` rebasados sobre `279cb8a0` (minusculas, sem emoji, sem co-author, corpos com `refs #111`, zero keywords de fechamento)

## Nao-objetivos

- Nao mexer em `src/api/server.rs` alem do estritamente necessario; o servidor esta correto.
- Nao adicionar dependencia nova (`AGENTS.md`: checar se as existentes cobrem antes).
- Nao tocar `src/app/api/panes.rs` nem `src/api/schema/panes.rs` — sao da folha #112, rodando em paralelo. Conflito ali e falha de coordenacao, nao de merge.
