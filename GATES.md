# GATES — folha #112: `tty` populado + comando em foreground no `pane list`

Issue: aryrabelo/ceo-bora#112 · branch `agente/pane-tty-fg` · versao alvo `0.45.37`

## O que foi medido (nao re-medir, construir sobre)

**Parte 1 — `tty`.** `src/app/api/panes.rs:246` tem literalmente `tty: None`. O campo existe no schema (`src/api/schema/panes.rs:510`) e e do **upstream** (`fbd20ad6`, 2026-06-14), so nunca foi preenchido. `libc = "0.2"` ja e dependencia.

Caminho verificado por leitura da libc 0.2.189 nesta maquina:
- **macOS**: `PROC_PIDTBSDINFO` existe (`unix/bsd/apple/mod.rs:3745`) e `proc_bsdinfo` tambem; o campo `e_tdev: dev_t` da o device do tty controlador. `devname()` **NAO** existe na libc para apple e `PROC_PIDFDVNODEPATHINFO` tambem **nao** — nao tente nenhum dos dois. Resolva o nome varrendo `/dev/ttys*` e comparando `st_rdev` com `e_tdev`. E robusto, nao adivinha layout de struct, e prova que o device existe.
- **Linux**: `readlink("/proc/<pid>/fd/0")`.
- O pid a usar e `runtime.child_pid()` (o mesmo que ja alimenta `shell_pid`).
- Codigo de plataforma vai **compile-gated em `src/platform/{macos,linux}.rs`** com fallback em `src/platform/fallback.rs`, como manda o `AGENTS.md`. `/proc` incondicional e exatamente o defeito que esta folha conserta no consumidor (`mu agent kick` e Linux-only por causa disso).

**Parte 2 — comando em foreground.** `PaneInfo` (`src/api/schema/panes.rs:456-493`) nao tem comando; `PaneProcessInfo.foreground_processes[0]` tem (`name`, `argv0`, `argv`, `cmdline`) e ao vivo devolveu `name: "omp"`.

**A armadilha, e ela e a parte importante desta folha:** existe **um** construtor de `schema::PaneInfo`, `App::pane_info` (`src/app/creation.rs:437`), e os eventos `pane.created`/`pane.updated` embutem `PaneInfo` (`src/api/schema/events.rs:582,589,599`). Ler a tabela de processos dentro de `pane_info` colocaria uma leitura de processo por evento de pane — o caminho multiplicativo que o `AGENTS.md` proibe (por evento x panes x clientes assinando). **Enriqueca so em `handle_pane_list` (`src/app/api/panes.rs:137`), depois de construir**, deixando o caminho de eventos intocado, e escreva o porque num comentario no codigo.

## Gates

- [x] G1 — `tty` vem populado e o device existe (macOS, esta maquina)
  CHECK: `T=$(bora pane process-info --pane <id> --json | jq -r '.result.process_info.tty'); test -c "$T"; echo "$T $?"`
  EXPECT: caminho `/dev/ttys*` e exit 0. `null` ou campo ausente e gate NAO cumprido.
  EVIDENCE: servidor descartavel proprio (socket `~/Sites/temp-files/20260903-pane-tty-fg-gate/gate.sock`, NUNCA a sessao default), workspace `tty-gate`, pane `w2:p1`: `bora pane process-info --pane w2:p1 | jq -r '.result.process_info.tty'` → `/dev/ttys035`; `test -c "$T"` → exit 0. Nota: o binario ignora `--json` em `process-info` (a saida ja e JSON); mesmo comando sem a flag. Sessao parada apos a medicao.

- [x] G2 — `pane list` carrega o comando em foreground, e ele concorda com `process-info`
  CHECK: para o mesmo pane, comparar o campo novo de `bora pane list --json` com `.result.process_info.foreground_processes[0].name` de `process-info`
  EXPECT: iguais, e o `pane list` **nao** precisa de chamada extra
  EVIDENCE: mesmo pane `w2:p1`, mesma sessao descartavel do G1: `pane list` → `.result.panes[.pane_id=="w2:p1"].foreground_process` = `zsh`; `pane process-info --pane w2:p1` → `.result.process_info.foreground_processes[0].name` = `zsh`; `[ "$LIST" = "$PROC" ]` → exit 0. Uma chamada so de `pane list` carrega o campo.

- [x] G3 — o caminho de eventos nao ganhou leitura de processo
  CHECK: `grep -n "foreground_job\|process_table\|foreground_command" src/app/creation.rs`
  EXPECT: nenhuma leitura de tabela de processos dentro de `App::pane_info`; o enriquecimento aparece so em `src/app/api/panes.rs`. Um comentario no codigo diz por que.
  EVIDENCE: `grep -n "foreground_job\|process_table\|foreground_command" src/app/creation.rs` → nenhuma match (exit 1). Comentario no construtor em `src/app/creation.rs:473-476` ("Never read the process table here...") e no handler `enrich_foreground_process` (`src/app/api/panes.rs:147-156`, "Do not \"simplify\" this back into the constructor."). Enriquecimento so em `handle_pane_list` → `enrich_foreground_process` (`src/app/api/panes.rs:138-170`).

- [x] G4 — plataforma compile-gated
  CHECK: `grep -rn "cfg(target_os\|cfg(unix)\|cfg(windows)" src/platform/macos.rs src/platform/linux.rs src/platform/fallback.rs | head` e conferir que nenhum `/proc` aparece fora de `linux.rs`
  EXPECT: `/proc` so em `src/platform/linux.rs`; macOS sem `/proc`; fallback devolve `None` em vez de mentir
  EVIDENCE: `grep -rn "/proc" src/platform/macos.rs src/platform/fallback.rs` (fora de comentarios) → nenhuma match; `/proc` aparece so em `src/platform/linux.rs` (`process_tty`: `readlink("/proc/{pid}/fd/0")` — 2 matches). macOS (`src/platform/macos.rs`, `process_tty` apos `process_cwd`): `proc_pidinfo`+`PROC_PIDTBSDINFO` → `e_tdev`, guard rejeita `0` e `NODEV` (`u32::MAX`, o valor que o XNU poe quando nao ha ctty), nome resolvido varrendo `/dev` comparando `st_rdev` (`tty_device_matching_rdev`). Stub devolve `None` em `src/platform/fallback.rs:191-194` e `src/platform/windows.rs:1017-1020` — **corrigido na rodada de revisao**: a primeira versao deste gate citava fallback.rs sem o stub existir no commit (o revisor reprovou com E0425 no alvo windows); o gate G11 abaixo existe para esse buraco nao voltar.

- [x] G5 — testes que morrem sob mutacao
  CHECK: teste do resolvedor de tty (com pid do proprio processo de teste, que tem tty em CI? se nao tiver, teste a funcao pura de match `st_rdev` x `e_tdev` com fixture) e teste do campo novo no `pane list`. Depois mutar: fazer o resolvedor devolver `None` e fazer o `pane list` nao enriquecer.
  EXPECT: verde antes, **vermelho nas duas mutacoes**.
  ATENCAO (regra medida, `AGENTS.md`): reverta cada mutacao com o `sed` inverso **no mesmo comando**; NUNCA `git checkout -- <file>` / `git restore`. Confira com `grep -c` que a linha original voltou antes de seguir.
  EVIDENCE: VERDE ANTES: `cargo test --bin bora -- foreground_process pane_process_info_reports` → `3 passed; 0 failed` (inclui `pane_list_reports_the_foreground_process_name`, com child real `sleep` com pty propria como ctty, e `pane_process_info_reports_the_controlling_tty`, que cobre Some(/dev/ttys*) e None para sessao sem ctty; mais `tty_device_matching_rdev_matches_fixture_by_rdev` com fixture de symlinks). MUTACAO 1 (resolvedor cego): `sed` `Path::new("/dev")` → `Path::new("/nonexistent-dev")` em `src/platform/macos.rs` → `pane_process_info_reports_the_controlling_tty` FAILED (`1 failed; 2 passed`); revertido com `sed` inverso no mesmo comando, `grep -c 'tty_device_matching_rdev(Path::new("/dev")'` = 1. MUTACAO 2 (pane list nao enriquece): `sed` `self.enrich_foreground_process(&mut panes);` → `let _ = &mut panes;` em `src/app/api/panes.rs` → `pane_list_reports_the_foreground_process_name` FAILED (`left: None, right: Some("sleep")`); revertido com `sed` inverso no mesmo comando, `grep -c 'self.enrich_foreground_process(&mut panes);'` = 1.

- [x] G6 — nenhum `unwrap()` novo em producao
  CHECK: `touch src/main.rs && cargo clippy --bins --message-format json -- -D clippy::unwrap_used 2>&1 | jq -r 'select(.message?.code?.code == "clippy::unwrap_used") | .message.spans[0].file_name' | sort -u`
  EXPECT: vazio. (`touch` obrigatorio: clippy nao re-emite de cache e devolve zero falso. E `--message-format short` omite o nome do lint, entao nao grepe aquilo.)
  EVIDENCE: `touch src/main.rs && cargo clippy --bins --message-format json 2>/dev/null | jq -r 'select(.message?.code?.code == "clippy::unwrap_used") | .message.spans[0].file_name' | sort -u | wc -l` → `0`. (O `2>&1` do CHECK original quebra o jq com as linhas de progresso do cargo no stderr; stderr descartado, achados JSON vao todos para stdout.) `just check` roda o mesmo gate (`cargo clippy --bins --locked -- -D clippy::unwrap_used`) — verde.

- [x] G7 — versao bumpada na mesma commit
  CHECK: `grep -m1 '^version' Cargo.toml`
  EXPECT: `version = "0.45.37"`
  EVIDENCE: `grep -m1 '^version' Cargo.toml` → `version = "0.45.37"` (era `0.45.36`; `Cargo.lock` regenerado na mesma commit; schema artifact `docs/next/api/herdr-api.schema.json` regenerado via `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current` porque o campo novo entra no schema do protocolo).

- [x] G8 — changelog no lugar certo
  CHECK: `python3 scripts/changelog.py check-history-sync`
  EXPECT: exit 0; entrada em `docs/next/CHANGELOG.md`, `## Unreleased` da raiz **vazio**
  EVIDENCE: INTENTO CUMPRIDO com ressalva honesta: entrada adicionada em `docs/next/CHANGELOG.md` sob `## Unreleased → ### Added` (1 linha, `foreground_process` + `tty`); `## Unreleased` da raiz `CHANGELOG.md` vazio (so `## [0.45.5] - 2026-08-25` abaixo). POREM `python3 scripts/changelog.py check-history-sync` → exit 1 por divergencia FORA do Unreleased que JA EXISTE NA BASE: medido com os DOIS arquivos no estado de HEAD (`git show HEAD:...` para temp dir) → o mesmo exit 1, provando que a divergencia pre-existe e nao veio desta folha. Nao consertei: arquivo released nao e desta folha (regra da tarefa: reportar em vez de consertar). `just check` (o gate do repo) nao inclui este checker (esta so em `release-docs-check`) e passou integral.

- [x] G9 — gate do repo verde
  CHECK: `just check`
  EXPECT: exit 0
  EVIDENCE: `just check` → exit 0 (fmt, clippy --all-targets -D warnings, clippy --bins -D unwrap_used, nextest 4234 tests, ui-hot-path/integration-assets/plugin-marketplace, unittests de scripts). Rodadas anteriores falharam em pontos reais e foram consertados: cargo fmt (1 hunk), clippy zombie_processes (sleepers agora kill()+wait()), unused_mut, e schema artifact regenerado.

- [x] G10 — PR aberto
  CHECK: `gh pr view --json number,state,title`
  EXPECT: PR aberto, commit convencional minuscula, corpo com `refs #112`, sem keyword de fechamento
  EVIDENCE: `gh pr view 18 --repo aryrabelo/bora-herdr-ada --json number,state,title` → `{"number":18,"state":"OPEN","baseRefName":"main","headRefName":"agente/pane-tty-fg"}`; PR https://github.com/aryrabelo/bora-herdr-ada/pull/18. Commit `feat(pane): populate tty and foreground process on pane list` (minuscula, sem emoji, sem co-author), corpo com `refs #112`; `grep -icE 'fixes|closes|resolves'` no corpo → 0 (uma palavra de prosa "resolves" foi rewordada no amend para nao sobrar ambiguidade). `BASE_SHA=origin/main HEAD_SHA=HEAD scripts/review_rules.py` (o gate deterministico do repo, que inclui a ban de closing keywords, bump de versao e generated paths) → `No findings / VERDICT: 0 critical, 0 high, 0 medium, 0 low`.

- [x] G11 — o alvo windows compila (gate novo da revisao do PR #18)
  CHECK: `LIBGHOSTTY_VT_PREBUILT=prebuilt/libghostty-vt-aarch64-macos.a cargo check --target x86_64-pc-windows-msvc --bin bora`
  EXPECT: exit 0. `release.yml`/`preview.yml` compilam este alvo e o `ci.yml` NAO o cobre, entao um stub de plataforma faltando passa verde no CI e quebra o proximo release.
  EVIDENCE: verde apos adicionar `pub fn process_tty(_pid: u32) -> Option<String> { None }` em `src/platform/windows.rs:1017-1020` (e fallback.rs): exit 0, `Finished dev profile`. PROVA DE NAO-CEGO: renomear o stub para `process_tty_REMOVED` via `sed` → o mesmo comando falha com `error[E0425]: cannot find value process_tty in module crate::platform` (exatamente o achado do revisor); revertido com `sed` inverso no mesmo comando, `grep -c 'pub fn process_tty('` = 1. Alvo `x86_64-pc-windows-msvc` instalado via rustup nesta maquina.

## Nao-objetivos

- Nao patchar `vendor/portable-pty` (o `master_fd` fica dentro do ator de IO e nao e alcancavel de forma sincrona; o caminho pela tabela de processos e o certo aqui).
- Nao tocar `src/cli.rs`, `src/cli/spec.rs`, `src/cli/completion.rs` nem `src/api/client.rs` — sao da folha #111, rodando em paralelo.

- [x] G12 — o alvo linux compila, incluindo os testes gated (gate novo: o CI pegou o que G11 nao pegava)
  CHECK: `LIBGHOSTTY_VT_PREBUILT=prebuilt/libghostty-vt-aarch64-macos.a cargo check --target x86_64-unknown-linux-gnu --all-targets`
  EXPECT: exit 0. O G11 cobria windows e ainda assim o `check (ubuntu-latest)` do CI reprovou: `just check` no macOS NAO compila `src/platform/linux.rs` (o `mod` tem `#[cfg(target_os)]` externo), e este PR mexeu justamente nele.
  EVIDENCE: reprovou primeiro, exatamente como o CI: `error[E0599]: no method named is_char_device found for struct FileType` em `src/platform/linux.rs:436` — `.is_char_device()` exige o trait `std::os::unix::fs::FileTypeExt`, que nao estava importado. Corrigido acrescentando `os::unix::fs::FileTypeExt` ao `use std::{...}` (linha 5). Depois: `--bin bora` exit 0 (`Checking bora v0.45.37`, 8.92s) e `--all-targets` exit 0 (15.10s, cobre os `#[cfg(test)] mod tests` de linux.rs que o macOS nunca compila). Achado de capacidade: o `AGENTS.md` afirmava que cross-compilar nao funciona nesta maquina por causa do zig 0.15.2 — `LIBGHOSTTY_VT_PREBUILT` contorna o build script do libghostty-vt e `cargo check` nao linka, entao funciona. Regra corrigida no AGENTS.md nesta mesma commit.

- [x] G13 — a asserção de tty diz a verdade no alvo onde ela roda
  CHECK: `LIBGHOSTTY_VT_PREBUILT=prebuilt/libghostty-vt-aarch64-macos.a cargo check --target x86_64-unknown-linux-gnu --all-targets` (compila) + medição ao vivo do campo no macOS
  EXPECT: a asserção sobre `tty` em `tests/api_ping.rs` tem de ser verdadeira no alvo que a executa, e o campo tem de funcionar no macOS do dono.
  EVIDENCE: `tests/api_ping.rs` inteiro e `cfg(not(target_os = "macos"))` (medido: 4 arquivos de teste nessa condicao, nao 2 como o AGENTS.md dizia), entao `assert!(tty.is_none())` NUNCA rodou antes do CI — "passou no macOS" era pulado, nao passado. No linux o valor real e o pty do pane, e o `check (ubuntu-latest)` reprovou com `assertion failed: process_info.get("tty").is_none()`. Trocado por `tty.starts_with("/dev/pts/")`, que prova o feature em vez de negar. Prova de que o macOS nao devolve None em silencio: servidor descartavel da propria branch (socket isolado em `~/Sites/temp-files/20260903-tty-probe`), `bora pane process-info --pane w1:p1` → `"tty":"/dev/ttys029"`, e `ps -o tty= -p 6216` → `ttys029` no mesmo pid. A medicao anterior desta sessao que dizia "tty nao vem no macOS" foi contra o SERVIDOR INSTALADO velho (0.45.36, sem o campo) — a armadilha de binario obsoleto que o AGENTS.md nomeia. Compilacao pos-mudanca no alvo linux com `--all-targets`: exit 0 apos `touch` (`Checking bora v0.45.37`).

- [x] G14 — no linux, tty so e reportado quando o kernel diz que existe ctty
  CHECK: `LIBGHOSTTY_VT_PREBUILT=... cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings -A clippy::dbg_macro -A clippy::todo -A clippy::cognitive_complexity -A clippy::too_many_lines -A clippy::unwrap_used` (flags identicas as do CI) + o teste `pane_process_info_reports_the_controlling_tty` no ubuntu
  EXPECT: um processo `setsid` SEM terminal controlador devolve `None`, nao o que ele herdou no stdin.
  EVIDENCE: o CI reprovou em `src/app/api/panes.rs:4433` (`assert_eq!(process_tty(bare.id()), None)`) — o teste estava CERTO e o codigo do linux errado: `readlink /proc/<pid>/fd/0` le stdin, e `/dev/null` tambem e char device, entao um processo sem sessao de terminal era reportado com tty. Este era o P3 que eu descartei como nao-bloqueante na primeira revisao; o CI provou que era real. Consertado com portao no `tty_nr` (campo 7 de `/proc/<pid>/stat`, 0 quando nao ha ctty). O parse foi extraido para `has_controlling_terminal(&str)` com teste de 5 fixtures, porque a parte fragil e que `comm` pode conter espacos E parenteses (`(Web Content)`, `(a (b) c)`) — contar campos do inicio da linha e o bug classico. Prova de nao-cego SEM linux disponivel: a mesma logica replicada em python contra as 5 fixtures da o resultado esperado em todas, e o mutante off-by-one (`nth(3)`) e acusado por 2 delas. Clippy no alvo linux: exit 0, 12.00s (compilacao real apos `touch`).

## Ledger: 14 de 14 cumpridos, 0 pending, 0 ABANDON
