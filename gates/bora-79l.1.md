# Gates — bora-79l.1 (F0): contrato executável

Scope: exportador que injeta a captura REAL da sidebar como bloco "hoje" no
HTML-contrato (lado a lado com o alvo), teste do exportador rodando no
`cargo test`, e o teste P4-A `#[ignore]` comparando captura vs alvo linha a
linha — o alvo que F1..F7 destravam.

- [x] G1: o teste do exportador (NÃO ignorado) prova que o bloco gerado
      embrulha toda linha capturada, escapa HTML (`<a &` vira entidade),
      coloriza spans reais (Rgb/Modifiers) e traz a coluna alvo do MESMO
      const que o P4-A usa.
  CHECK: cargo nextest run -E 'test(exporter_)'
  EXPECT: /2 passed/
  EVIDENCE: ──────────── | Summary [   0.009s] 2 tests run: 2 passed, 4151 skipped

- [x] G2: P4-A existe, é `#[ignore]` (a suíte fica verde com ele ligado) e
      está VIVO: rodado com `--ignored` ele falha hoje contra o const do
      alvo com diff de linha — alvo real, não tautologia.
  CHECK: cargo test --locked --bin bora p4a 2>&1 | tail -2
  EXPECT: /1 ignored/
  EVIDENCE: (`--ignored`) FAIL na linha 01 — `assertion left == right failed:
  row 01 diverges from the contract / left: "Bora ...8/8" / right: " Bora
  ...8/8"` — hoje o código pinta o nome na col 0, o alvo pede col 0 em
  branco. Vivo, não tautologia.

- [x] G3: o const do alvo é o contrato de 35 linhas extraído MECANICAMENTE
      do HTML (clusters pinados na col 56, bolinhas l2 com 1 espaço,
      LIVRE em branco), pinado por teste.
  CHECK: cargo nextest run -E 'test(alvo_const)'
  EXPECT: /1 passed/
  EVIDENCE: ──────────── | Summary [   0.009s] 1 test run: 1 passed, 4152 skipped

- [x] G4: `just sidebar-preview` escreve o bloco "hoje" entre os marcadores
      `sidebar-preview:begin/end` no HTML-contrato, idempotente.
  CHECK: just sidebar-preview && grep -c 'sidebar-preview:begin' .local/prd/sidebar-project-view-anatomy.html
  EXPECT: 1
  EVIDENCE: grep -c → `1`; segundo run byte-idêntico (md5
  2244d369589b81fc07c6067ca22d3d80 antes e depois) — idempotente.

- [x] G5: a suíte de captura inteira verde com P4-A ignorado (aceite do
      plano em forma de suíte).
  CHECK: cargo test --locked --bin bora ui::sidebar::capture:: 2>&1 | tail -2
  EXPECT: /test result: ok/
  EVIDENCE: test result: ok. 12 passed; 0 failed; 2 ignored; 0 measured; 4034 filtered out; finished in 0.02s

- [x] G6: nada de código de produção — todo Rust novo sob `#[cfg(test)]`
      (o módulo capture inteiro já é); mudanças limitadas a capture.rs,
      justfile e o HTML (.local é ignorado pelo git).
  CHECK: git status --porcelain
  EXPECT: /capture.rs/
  EVIDENCE: ?? 2026-08-27T14-40-49-589Z_01a043aa-9bb5-7539-bfc1-68b97539eb26/ | ?? gates/bora-79l.1.md
