# Gates — bora-79l.2 (F1): modelo Section

Scope: `Section{kind, header_on, parts, children}` com
`SectionKind{Branch, Comando, Checks, Livre}` + `SectionParts{dots, diff}` +
parse/serialize YAML stub (serde_yaml_ng, convenções do persist::projects).
Sem wiring de render (F2/F3) nem de projects.yml (F7).

- [x] G1: os três testes do modelo passam — round-trip do formato (aceite do
      plano), defaults preenchendo campos omitidos (header ON, parts ON),
      e rejeição de campo desconhecido.
  CHECK: cargo nextest run -E 'test(sections_model)'
  EXPECT: /3 passed/
  EVIDENCE: ──────────── | Summary [   0.011s] 3 tests run: 3 passed, 4153 skipped

- [x] G2: o módulo novo não suja o build — zero warnings de dead_code
      (o `#[allow(dead_code)]` tem justificativa citando F2/F3/F7).
- [x] G3: bump de versão no mesmo commit (Cargo.toml 0.45.10 → 0.45.11) e
      Cargo.lock regenerado.
  EVIDENCE: commit 0896e7fe contém Cargo.toml (`version = "0.45.11"`,
  1 linha trocada) e Cargo.lock (1 linha trocada) — `git show --stat
  0896e7fe`; review_rules.py sobre o commit: "No findings".

