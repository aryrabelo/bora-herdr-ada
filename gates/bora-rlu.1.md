# Gates — bora-rlu.1 — truncate header and branch labels with explicit ellipsis

Lead dispatch did not include a pre-created gates file; ledger created by the
builder from the bead's verbatim acceptance criteria. Evidence lines reference
the final working tree.

## Gate 1 — No raw-clipped label
- `ProjectHeaderBranch.label` wrapped in `truncate_end`: src/ui/sidebar.rs:2105
  (`format!("[{}]", truncate_end(&b.label, avail))`), budget at :2097-2101
  (`used` = sum of `display_width` over the row's actual spans — rail + name +
  gap + ahead/behind + PR badge + collapse dot — same arithmetic as the
  Workspace arm's `avail`; +2 for the `[` `]`).
- `BranchHeader.label` wrapped in `truncate_end`: src/ui/sidebar.rs:2258
  (`Span::styled(truncate_end(label, avail), name_style)`), budget at :2251-2255
  (connector + indent + tab dots + ahead/behind + idle age / PR badge).
- Both arms build all other spans first, measure with `display_width`, then
  insert the truncated label span at its recorded index — the budget subtracts
  the row's real chrome, not a guessed constant.
- EVIDENCE: src/ui/sidebar.rs:2091-2109 and src/ui/sidebar.rs:2247-2259.

## Gate 2 — Unit tests pin output at 20/30/40 cols
- `project_header_branch_label_truncates_with_explicit_ellipsis`
  (src/ui/sidebar.rs:3872-3902): renders the folded-branch ProjectHeader at
  20/30/40 cols and asserts the exact row strings
  `╭─herdr [feature/b…]`, `╭─herdr [feature/branch-name…]`,
  `╭─herdr [feature/branch-name-longer-th…]`.
- `branch_header_label_truncates_with_explicit_ellipsis`
  (src/ui/sidebar.rs:3905-3944): renders a plain BranchHeader at 20/30/40 cols
  and asserts `├── feature/branch-…`, `├── feature/branch-name-longe…`,
  `├── feature/branch-name-longer-than-sid…`.
- Full-row `assert_eq!` against literal strings (never `len <= width`), so both
  tests go red if `truncate_end` is removed from either arm (the raw label is
  hard-clipped by ratatui without the trailing `…`).
- EVIDENCE: src/ui/sidebar.rs:3872, src/ui/sidebar.rs:3905, expected literals
  at :3886-3888 and :3928-3930.

## Gate 3 — Repo-view tests green
- Not runnable by the builder (dispatch forbids cargo). Static inspection:
  every existing sidebar render test that exercises ProjectHeader/BranchHeader
  uses short labels (`main`, `init`, `feat/a`…) at ≥24-col areas where
  `truncate_end` is a no-op (returns the input unchanged when it fits), so
  their assertions are unaffected. The only long-label render test
  (`workspace_list_truncates_cjk_branch_without_panic`, src/ui/sidebar.rs:3772)
  asserts no-panic only.
- EVIDENCE: pending — lead runs `cargo test` (gate was inspection-only here).

## Non-goals respected
- Indented-workspace-row label logic (`full_label` / Workspace arm,
  src/ui/sidebar.rs:2437 area) untouched — that is bora-rlu.2.
- Row heights/counts untouched; `entry_row_height` and the lockstep contract
  (src/ui/sidebar.rs:684-686) unmodified — label-content change only.
- The mouse-capture `" + "` overlay is not subtracted from the budget: it is an
  overlay painted after the row (same coexistence with clipping as before),
  not row chrome, and the Workspace arm's budget does not model one either.
