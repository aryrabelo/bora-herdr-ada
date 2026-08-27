# Gates — bora-79l.2 (F1): modelo Section

Scope: `Section{kind, header_on, parts, children}` com
`SectionKind{Branch, Comando, Checks, Livre}` + `SectionParts{dots, diff}` +
parse/serialize YAML stub (serde_yaml_ng, convenções do persist::projects).
Sem wiring de render (F2/F3) nem de projects.yml (F7).

- [ ] G1: os três testes do modelo passam — round-trip do formato (aceite do
      plano), defaults preenchendo campos omitidos (header ON, parts ON),
      e rejeição de campo desconhecido.
  CHECK: cargo nextest run -E 'test(sections_model)'
  EXPECT: /3 passed/
  EVIDENCE: pending

- [ ] G2: o módulo novo não suja o build — zero warnings de dead_code
      (o `#[allow(dead_code)]` tem justificativa citando F2/F3/F7).
  CHECK: cargo check 2>&1 | grep -cE 'never (used|constructed)'
  EXPECT: 0
  EVIDENCE: pending

- [ ] G3: bump de versão no mesmo commit (Cargo.toml 0.45.10 → 0.45.11) e
      Cargo.lock regenerado.
  EVIDENCE: pending
