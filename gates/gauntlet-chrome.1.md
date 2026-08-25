# Bar + gates — sidebar chrome round

The bar is **Solo's sidebar** (soloterm.com), catalogued from its real product
screenshots in `.local/prd/sidebar-gauntlet.md` §1. The owner's words: *mais
bonito, dinâmico e plugin ready*.

Solo is the CRITERIA SOURCE, not the comparison artifact. It is a native GUI and
we are a TUI, so a blind verdict between them would judge the medium, not the
quality. The comparison artifact is our own rendering before this round, saved at
`~/Sites/temp-files/gauntlet/baseline.txt`, same fixture, same 56x40.

## Measured gaps, from the baseline capture — not from reading code

Four states, one identical row colour, `mod:NONE` on all four:

    row 10 (finished, unseen)  name fg Rgb(166,173,200) mod:NONE
    row 12 (working)           name fg Rgb(166,173,200) mod:NONE
    row 14 (idle, seen)        name fg Rgb(166,173,200) mod:NONE
    row 16 (no agent)          name fg Rgb(166,173,200) mod:NONE

Row 08, the blocked one, was bold and brighter — but only because it was the
active row, so an inactive blocked row differed from an idle one by one glyph.

## Gates

- [x] GC1: an inactive row's colour and weight follow its agent state.
  EVIDENCE: the row colour is now taken from `state_dot`'s own fg
      (`dot_style.fg`, `sidebar.rs:3149`) rather than a second hand-picked
      palette, so the text literally follows the dot instead of agreeing with it
      by coincidence. Weight is three tiers ranked through
      `crate::detect::attention_priority` against `Working`: BOLD for the two
      states that want you, NONE for working, DIM for idle-seen and no-agent.
      Lead-verified off the regenerated capture, not the report: row 08
      `4..8=fg:Rgb(243,139,168),mod:BOLD` (was `Rgb(205,214,244)`), row 10
      `fg:Rgb(249,226,175),mod:BOLD` (was `Rgb(166,173,200),mod:NONE`), row 12
      `fg:Rgb(108,112,134),mod:NONE`, rows 14/16 `mod:DIM`.

- [x] GC2: the distinction survives losing colour.
  EVIDENCE: verified by stripping every `fg:` term from the capture's style lines
      — which is what serializing fg, bg and modifier as separate fields is for.
      The gate's named pair, blocked versus working, is BOLD versus NONE:
      distinguishable by weight alone, no colour. **Honest residual, and it is
      the one the blind critic independently identified:** weight alone yields
      three buckets for five states, so two pairs collide — blocked with
      finished-unseen (both BOLD) and idle-seen with no-agent (both DIM). Those
      collisions are defensible rather than accidental, because each colliding
      pair is semantically adjacent: both members of the first want you, both
      members of the second want nothing. Within a pair the glyph still separates
      them (◆ vs ⠁, ○ vs ◰), so five of five remain distinguishable without
      colour; it is the RANKING between the two "wants you" states that needs
      hue. Recorded here rather than left implicit.

- [x] GC3: the active row is marked without repainting its whole background.
  EVIDENCE: the full-row background fill is now gated on
      `selected || is_dragged` rather than `selected || is_active || is_dragged`,
      so an active-but-not-navigating row gets `bg:Reset` throughout and instead
      a one-column left-edge lane, blank on every row and `▎` in the accent
      colour on the active one. Verified in the capture: row 08 is
      `0..1=fg:Rgb(137,180,250)` with no `bg:` anywhere on the row. The gate's
      second clause — that this must not make the active row's own state harder
      to read — is what `default_space_workspace_style_tracks_active_state`
      asserts: active and inactive get identical fg and weight, only the marker
      differs. Because the background is untouched, the blocked active row now
      reads red-bold on its own merit rather than being repainted by selection.

- [x] GC4: nothing added to the render path allocates, does I/O, inspects
      processes, or formats a terminal snapshot.
  EVIDENCE: the new work is `ws.aggregate_state`, `state_dot` and
      `attention_priority` — pure in-memory folds already called in this same
      function. One caveat the builder raised unprompted and correctly:
      `project_row_line`'s new inline rule reuses the pre-existing
      `project_row_trailing` fill, which does one `String::repeat` per row when a
      fill is supplied. That is the same already-shipped pattern
      `section_header_line` uses for COMMANDS and CHECKS, now also applied to the
      project row, and it is bounded by visible row count rather than multiplied
      by panes or clients. Not a new violation, but it is an allocation per
      visible row and it belongs in the record rather than in a footnote.

- [x] GC5: the lockstep holds and no row height changed.
  CHECK: cargo nextest run --locked -E 'test(/lockstep/)'
  EXPECT: /0 failed/
  EVIDENCE: `4 tests run: 4 passed`. `entry_row_height` untouched, still 1 for
      every variant, no new arm. The separator (Solo #11) was deliberately built
      as an inline rule on the project row rather than as its own row, which is
      the choice that keeps this gate green — a standalone separator row would
      have meant a new `WorkspaceListEntry` variant threaded through all three
      passes and every exact-sequence test. Rejected for that reason and the
      reason is stated, which is what the gate asked for.

- [x] GC6: the capture regenerated and judged.
  EVIDENCE: regenerated and diffed against the baseline: 24 changed lines, small
      and legible rather than a total reflow, which is the property the harness
      was built for. Visible changes: an inline rule on the project row, the `▎`
      active marker in a dedicated column 0 gutter, agent rows realigned from
      indent 1 to indent 2 so they sit under their branch, and state-following
      colour and weight on every agent row.

- [x] GC7: two-sided tests, mutation-proved.
  EVIDENCE: five builder mutations plus one re-run independently by the lead
      rather than trusted: `sidebar.rs:3149`, `dot_style.fg.unwrap_or(p.subtext0)`
      → `p.subtext0`, which severs the row colour from the dot while leaving
      everything else intact. Reddened
      `default_space_workspace_style_tracks_active_state` and nothing else — a
      precise, single-test failure, which is what a good mutation looks like.
      Restore `cmp` byte-identical. 163 sidebar tests pass.

- [x] GC8: production `unwrap()` zero, `#[allow]` justified.
  CHECK: touch src/main.rs && cargo clippy --bins --locked 2>&1 | grep -c "clippy::unwrap_used"
  EXPECT: 0
  EVIDENCE: 0, with `touch` first so clippy does not replay a cached green.

- [x] GC9: the round passes a BLIND comparison, which is the point of running it
      as a gauntlet rather than as a self-assessment.
  EVIDENCE: two independent critics, each given the two renderings unlabeled as
      SIDEBAR ONE and SIDEBAR TWO with the new one placed first so ordering
      carried no hint, each told explicitly not to reward density and not to
      decline a choice. Both picked the new rendering, and — the part that makes
      the verdict worth having — on DIFFERENT grounds, neither of which was
      handed to them as the answer. One judged state legibility and did the
      arithmetic itself: three weight buckets versus two, with four of five
      states byte-identical on both fg and mod in the old rendering. The other
      judged alignment and calm: agent rows now sit under their parent branch
      instead of one column shallower, column 0 is a consistent gutter rather
      than incidental space, and the active row is marked without a full-row
      fill. Two judges, two criteria, one winner.
