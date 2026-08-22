# Gates — bora-7c5.2 — composer com borda, título `[ Chat ]` e contador de caracteres

Lead dispatch did not include a pre-created gates file; ledger created by the
builder from the bead's verbatim acceptance criteria. Evidence lines reference
the final working tree. `cargo` was not run (dispatch forbids it) — the lead
runs the gates.

## Gate 1 — Frame reuses the panel shell, not a second border style
- `render_input` draws the frame through `render_panel_shell` (the same helper
  the chat popup, prompts, menus, navigator and dialogs use), accent border on
  `panel_bg`: src/ui/chat.rs:394.
- The bead text's "render_panel_shell (src/ui/chat.rs:38)" line number had
  drifted: line 38 is a *call site*; the symbol lives at src/ui/widgets.rs:11.
  Located by symbol as instructed; no architecture-doc change needed.

## Gate 2 — Title and counter on the top border, derived at render time
- Title ` [ Chat ] ` painted left-aligned after the top-left corner, accent +
  BOLD: src/ui/chat.rs:399-409 (const at src/ui/chat.rs:28).
- Counter `{chars} ` right-aligned, ending one column before the top-right
  corner: src/ui/chat.rs:410-420. Value is `app.chat.input.chars().count()`
  computed inside `render()` — nothing stored, no new `ChatViewState` field,
  `render()` stays pure (AGENTS.md render rule).

## Gate 3 — Border arithmetic traced through every area computation
Places checked, one shared constant (`COMPOSER_FRAME_ROWS = 3`,
src/app/input/overlays.rs:26) drives all of them:
- `chat_input_rect`: owns the bottom 3 inner rows, full inner width, flush
  with the inner bottom (src/app/input/overlays.rs:823-832).
- `chat_channel_list_rect`: height `inner - 3` (src/app/input/overlays.rs:779).
- `chat_members_rect`: height `inner - 3` (src/app/input/overlays.rs:790).
- `chat_messages_rect`: body `inner - (2 header + 3 frame)` (was `inner - 4`)
  (src/app/input/overlays.rs:805). `chat_messages_viewport`/`chat_max_scroll`
  (src/app/input/chat.rs:716-723) and `chat_message_hit_at` (:755-762) derive
  from this rect, so scroll math and click mapping follow automatically.
- `chat_header_rect` (overlays.rs:809-812) is relative to the messages rect —
  unchanged semantics.
- `chat_new_channel_rect` / `chat_add_member_rect` / `chat_member_remove_x` /
  `chat_members_hit_at`: all relative to their column rects (which already
  shrank) — no absolute rows anywhere.
- `render_column_separator` uses `left.height` — follows the columns.
- The prompt sub-mode (`chat_prompt_rect` family) is a centered popup over the
  overlay, independent of the composer rows.
- Lockstep test `chat_column_rects_stop_where_the_composer_frame_begins`
  (src/ui/chat.rs:1027-1063) pins the agreement at 106x20, 60x24, 36x20 and
  the degenerate 30x8: every column that still renders ends exactly where the
  composer begins, and the header tiles onto the timeline body.

## Gate 4 — Tests go red without the change
- `composer_renders_frame_title_and_counter` (src/ui/chat.rs:962-997): full
  exact-row `assert_eq!` on all three frame rows (top border with title +
  counter `0 `, input row, bottom border) inside the popup border. Without
  the shell/title/counter the rows are plain text — mismatch.
- `composer_counter_tracks_the_draft_length` (src/ui/chat.rs:999-1021):
  "hi" → `2 `, "hello there" → `11 `, both asserted as full border rows —
  the dash run shrinks as the counter widens, so a stale or missing counter
  cannot pass.
- `chat_column_rects_stop_where_the_composer_frame_begins`: with the old
  1-row input rect the height assertion fails; with any *single* rect left on
  the old bottom edge the column-bottom assertion fails (the lead's
  one-rect-reverted mutation scenario).
- EVIDENCE: pending — lead runs `cargo test chat` and the mutations.

## Gate 5 — Existing tests audited, none needed loosening
- All existing chat tests use rect-relative math (`area.y + …`, `list.y +
  list.height - 1`, `members.y + 1 + idx`), so the two-row timeline loss moves
  nothing: `the_plus_row_opens_the_prompt_and_is_not_a_channel_row`
  (src/app/input/chat.rs:1466-1467), members hits (:1710, :1958), marker click
  (:1800), timeline-geometry-exists check (:1781-1784).
- No test asserted absolute composer/timeline rows before; none was edited,
  loosened, or deleted.

## Gate 6 — Repaint contract
- The new geometry is derived purely at render time from the terminal size
  (`view.sidebar_rect.union(view.terminal_area)` → popup → inner); no
  runtime-mutable state feeds it, the frame is unconditional (no toggle), so
  no `AppState` mutation can reflow these rects without the outer dimensions
  changing — which already trips the full-repaint path the encoders check.
  No new `request_full_repaint()` call required (the 7c5.1 expansion toggle
  keeps its own, src/app/input/chat.rs:744).

## Gate 7 — `just check` / `cargo test chat` verde
- EVIDENCE: pending — lead runs it (builder may not run cargo).
