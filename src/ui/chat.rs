//! Chat view render — senpai-style three columns inside bora's panel shell:
//! channel list | message timeline | member list, with a composer line.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::{
    status::{agent_icon, state_label_color},
    text::{display_width, middle_elide},
    widgets::{panel_contrast_fg, render_panel_shell},
};
use crate::api::schema::AgentStatus;
use crate::app::state::{AppState, ChatPrompt, ChatViewState, Palette};

const TIME_WIDTH: usize = 5; // "HH:MM"
/// Left indent before the "HH:MM" column on a message's first line, and
/// the same width every continuation/header-separator line reuses via
/// `MESSAGE_GUTTER` so wrapped text always lines up under the timestamp.
const MESSAGE_INDENT: usize = 2;
/// Gap between "HH:MM" and whatever follows (destination token or text).
const TIME_GAP: usize = 2;
/// Full message-line gutter: 2-col indent + 5-col "HH:MM" + 2-col gap = 9.
/// Replaces the old 21-col right-aligned sender column (`SENDER_WIDTH` +
/// `TIME_WIDTH` + `COLUMN_GAP*2`) now that a message's sender lives once
/// in its block's header line instead of repeating on every message row
/// (ceo-bora#38: grouped-by-sender Messages column).
const MESSAGE_GUTTER: usize = MESSAGE_INDENT + TIME_WIDTH + TIME_GAP;
/// Bottom row of the channel column: click (or Ctrl+N) to create a channel.
/// Same `+` vocabulary the sidebar uses for "new worktree" / "run command".
pub(crate) const NEW_CHANNEL_LABEL: &str = "+ new channel";
/// Caption painted on the composer's top border (GOAL 2026-08-22 §12.2):
/// the input line reads as a control, not floating text.
const COMPOSER_TITLE: &str = "[ Chat ]";
/// Caption on the channel column's border (GOAL 2026-08-22 §12.3): borders
/// plus a title replace the old single-character column separator.
const CHANNELS_TITLE: &str = "[ Channels ]";
/// Caption on the timeline column's border — same bracketed format as
/// `COMPOSER_TITLE`, distinct word so the transcript and the input control
/// don't share a caption.
const MESSAGES_TITLE: &str = "[ Messages ]";
/// Caption on the member column's border.
const MEMBERS_TITLE: &str = "[ Members ]";

/// Draws a chat column's frame: border via `render_panel_shell` plus a
/// bracketed caption painted inside the top border's corners — the same
/// treatment the composer introduced (bora-7c5.2). Every bordered column in
/// the chat view goes through this one function, so there is exactly one
/// border style and one caption layout to keep in sync.
fn render_column_frame(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    border_color: Color,
    bg: Color,
) -> Option<Rect> {
    let inner = render_panel_shell(frame, area, border_color, bg)?;
    let caption = format!(" {title} ");
    let caption_width = display_width(&caption) as u16;
    frame.render_widget(
        Paragraph::new(caption).style(
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(
            area.x + 1,
            area.y,
            area.width.saturating_sub(2).min(caption_width),
            1,
        ),
    );
    Some(inner)
}

fn agent_state(status: AgentStatus) -> crate::detect::AgentState {
    match status {
        AgentStatus::Idle | AgentStatus::Done => crate::detect::AgentState::Idle,
        AgentStatus::Working => crate::detect::AgentState::Working,
        AgentStatus::Blocked => crate::detect::AgentState::Blocked,
        AgentStatus::Unknown => crate::detect::AgentState::Unknown,
    }
}

pub(super) fn render_chat_overlay(app: &AppState, frame: &mut Frame) {
    let popup = app.chat_popup_rect();
    let Some(_inner) = render_panel_shell(frame, popup, app.palette.accent, app.palette.panel_bg)
    else {
        return;
    };
    let p = &app.palette;

    let channel_panel = app.chat_channel_list_panel_rect();
    let messages_panel = app.chat_messages_panel_rect();
    let members_panel = app.chat_members_panel_rect();

    render_column_frame(frame, channel_panel, CHANNELS_TITLE, p.surface1, p.panel_bg);
    render_column_frame(
        frame,
        messages_panel,
        MESSAGES_TITLE,
        p.surface1,
        p.panel_bg,
    );
    if members_panel.width > 0 {
        render_column_frame(frame, members_panel, MEMBERS_TITLE, p.surface1, p.panel_bg);
    }

    let list = app.chat_channel_list_rect();
    let messages = app.chat_messages_rect();
    let members = app.chat_members_rect();
    let input = app.chat_input_rect();

    render_channel_list(app, frame, list);
    render_messages(app, frame, messages);
    if members.width > 0 {
        render_members(app, frame, members);
    }
    render_input(app, frame, input);
    // Last: the prompt is modal, so it draws over every column.
    render_chat_prompt(app, frame);
}

/// The chat view's modal sub-mode: one small centered panel over the overlay,
/// shared by both prompts so the two cannot drift apart visually.
fn render_chat_prompt(app: &AppState, frame: &mut Frame) {
    let Some(prompt) = app.chat.prompt.as_ref() else {
        return;
    };
    let Some(rect) = app.chat_prompt_rect() else {
        return;
    };
    let Some(_inner) = render_panel_shell(frame, rect, app.palette.accent, app.palette.panel_bg)
    else {
        return;
    };
    let p = &app.palette;
    let (title, typed) = match prompt {
        ChatPrompt::NewChannel { input } => ("new channel", input.as_str()),
        ChatPrompt::AddMember { query, .. } => ("add agent", query.as_str()),
    };
    let Some(text) = app.chat_prompt_text_rect() else {
        return;
    };
    frame.render_widget(
        Paragraph::new(middle_elide(title, text.width as usize))
            .style(Style::default().fg(p.accent).add_modifier(Modifier::BOLD)),
        Rect::new(text.x, text.y.saturating_sub(1), text.width, 1),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "> ",
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate_input(typed, text.width.saturating_sub(3) as usize),
                Style::default().fg(p.text),
            ),
        ])),
        text,
    );
    let ChatPrompt::AddMember { selected, .. } = prompt else {
        return;
    };
    let Some(rows) = app.chat_prompt_rows_rect() else {
        return;
    };
    let candidates = app.chat_prompt_candidates();
    if candidates.is_empty() {
        frame.render_widget(
            Paragraph::new("no agent matches").style(Style::default().fg(p.overlay0)),
            Rect::new(rows.x, rows.y, rows.width, 1),
        );
        return;
    }
    let start = app.chat_prompt_window_start();
    for (offset, candidate) in candidates
        .iter()
        .skip(start)
        .take(rows.height as usize)
        .enumerate()
    {
        let highlighted = start + offset == *selected;
        let style = if highlighted {
            Style::default().bg(p.accent).fg(p.panel_bg)
        } else {
            Style::default().fg(panel_contrast_fg(p))
        };
        let detail = match (candidate.status.as_str(), candidate.cwd.as_deref()) {
            ("", None) => String::new(),
            ("", Some(cwd)) => format!("  {cwd}"),
            (status, None) => format!("  {status}"),
            (status, Some(cwd)) => format!("  {status}  {cwd}"),
        };
        let label = middle_elide(
            &format!("{}{detail}", candidate.name),
            rows.width.saturating_sub(1) as usize,
        );
        // Same selection vocabulary as the channel column.
        let content = if highlighted {
            format!("▐{label}")
        } else {
            format!(" {label}")
        };
        frame.render_widget(
            Paragraph::new(content).style(style),
            Rect::new(rows.x, rows.y + offset as u16, rows.width, 1),
        );
    }
}

fn render_channel_list(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let p = &app.palette;
    // The `+` row owns the last line of the column, so the channel rows get
    // one fewer. Geometry lives in `chat_new_channel_rect` — both agree or
    // clicks land on the wrong row.
    let new_channel = app.chat_new_channel_rect();
    let rows = area.height.saturating_sub(new_channel.height);
    for (idx, channel) in app.chat.channels.iter().enumerate().take(rows as usize) {
        let selected = idx == app.chat.selected;
        let never_messaged = channel.last_message_seq == 0;
        let style = if selected {
            Style::default().bg(p.accent).fg(p.panel_bg)
        } else if never_messaged {
            Style::default().fg(p.overlay0)
        } else {
            Style::default().fg(panel_contrast_fg(p))
        };
        // "never" is the same width as `short_time`'s "HH:MM" so the badge
        // column doesn't jitter between messaged and never-messaged rows.
        let activity = channel
            .last_message_ts
            .as_deref()
            .map(short_time)
            .unwrap_or("never");
        let detail = format!(
            "  {}·{} {activity}",
            channel.pane_count, channel.agent_count
        );
        // Unread marker (bora-7c5.4): two distinct "unread" notions exist
        // in this codebase. `ChannelSummary.unread` is nominally the
        // per-member mailbox count, but by the time it reaches this render
        // function `apply_chat_seen_cursor` (app/input/chat.rs) has already
        // overwritten it with the window's own view state — rooms with
        // messages newer than what THIS chat window has displayed
        // (`ChatViewState::seen`). That's the one a human staring at the
        // channel list wants, not the persisted per-member cursor, so this
        // render function reuses the field as-is rather than adding a
        // parallel mechanism. A read channel renders nothing: no empty
        // bracket, no "0", nothing to learn to ignore — that asymmetry is
        // the whole point. The badge is a separate, fixed-width span
        // appended after the elided name/detail so it's never swallowed by
        // `middle_elide`'s "…" and stays visually distinct (teal, bold)
        // from the pane/agent counts — it's rendered as its own span, not
        // concatenated into that text.
        let unread_badge = if channel.unread > 0 {
            format!(" {}●", channel.unread)
        } else {
            String::new()
        };
        let badge_width = display_width(&unread_badge);
        let label = middle_elide(
            &format!("{}{detail}", channel.name),
            area.width
                .saturating_sub(1)
                .saturating_sub(badge_width as u16) as usize,
        );
        let prefix = if selected { "▐" } else { " " };
        let mut spans = vec![Span::styled(format!("{prefix}{label}"), style)];
        if !unread_badge.is_empty() {
            let badge_style = if selected {
                style
            } else {
                Style::default().fg(p.teal).add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(unread_badge, badge_style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(area.x, area.y + idx as u16, area.width, 1),
        );
    }
    if app.chat.channels.is_empty() && rows > 0 {
        frame.render_widget(
            Paragraph::new("no channels").style(Style::default().fg(p.overlay0)),
            Rect::new(area.x, area.y, area.width, rows),
        );
    }
    if new_channel.height > 0 {
        frame.render_widget(
            Paragraph::new(middle_elide(
                NEW_CHANNEL_LABEL,
                new_channel.width.saturating_sub(1) as usize,
            ))
            .style(Style::default().fg(p.overlay1)),
            new_channel,
        );
    }
}

fn render_messages(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let p = &app.palette;
    let header = app.chat_header_rect();
    if header.height > 0 {
        let counts = app
            .chat
            .channels
            .get(app.chat.selected)
            .map(|channel| {
                format!(
                    "{} panes · {} agents",
                    channel.pane_count, channel.agent_count
                )
            })
            .unwrap_or_default();
        let title = format!(
            "{}  {}",
            app.selected_chat_channel_name().unwrap_or("no channel"),
            counts
        );
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                middle_elide(&title, header.width.saturating_sub(1) as usize),
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            )])),
            header,
        );
        let divider = Rect::new(header.x, header.y + 1, header.width, 1);
        if divider.height > 0 {
            frame.render_widget(
                Paragraph::new("─".repeat(divider.width as usize))
                    .style(Style::default().fg(p.surface1)),
                divider,
            );
        }
    }
    let lines = chat_display_lines(&app.chat, &app.palette, area.width);
    let start = app.chat.scroll.min(lines.len());
    let end = lines.len().min(start + area.height as usize);
    let visible: Vec<Line<'static>> = lines[start..end].to_vec();
    frame.render_widget(Paragraph::new(visible), area);
}

/// Right-edge remove control of a member row. Removal requires landing on
/// this glyph — clicking the name mentions the member instead, so a stray
/// click never ejects anyone.
const REMOVE_GLYPH: &str = "×";
/// Footer affordance of the members column.
const ADD_MEMBER_LABEL: &str = "+ add agent";

/// Live status label of an agent, shared by the members column and the
/// add-member candidate rows.
pub(crate) fn agent_status_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "",
    }
}

fn render_members(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let p = &app.palette;
    let header = format!("{} members", app.chat.members.len());
    frame.render_widget(
        Paragraph::new(header).style(Style::default().fg(p.overlay0)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    // Header row plus the "+ add agent" footer row bracket the member rows.
    let member_rows = area.height.saturating_sub(2) as usize;
    for (idx, member) in app.chat.members.iter().enumerate().take(member_rows) {
        let row = Rect::new(area.x, area.y + 1 + idx as u16, area.width, 1);
        let state = member.agent_status.map(agent_state);
        let (icon, icon_style) = match state {
            Some(state) => agent_icon(
                state,
                true,
                app.spinner_tick,
                app.status_indicators,
                &app.palette,
                None,
            ),
            None => ("·", Style::default().fg(p.overlay0)),
        };
        let name = member
            .name
            .clone()
            .unwrap_or_else(|| member.pane_id.clone());
        let status = member.agent_status.map(agent_status_label).unwrap_or("");
        // Icon + gap on the left, gap + remove control on the right.
        let text_width = area.width.saturating_sub(4) as usize;
        let status_width = if status.is_empty() {
            0
        } else {
            display_width(status) + 1
        };
        let name_text = middle_elide(&name, text_width.saturating_sub(status_width));
        let mut spans = vec![
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(
                name_text.clone(),
                Style::default().fg(match state {
                    Some(state) => state_label_color(state, true, &app.palette),
                    None => p.overlay0,
                }),
            ),
        ];
        if status_width > 0 {
            spans.push(Span::styled(
                format!(" {status}"),
                Style::default().fg(p.overlay0),
            ));
        }
        let used = display_width(&name_text) + status_width;
        spans.push(Span::raw(" ".repeat(text_width.saturating_sub(used) + 1)));
        spans.push(Span::styled(REMOVE_GLYPH, Style::default().fg(p.overlay0)));
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
    }
    let footer = app.chat_add_member_rect();
    if footer.height > 0 && footer.width > 0 {
        frame.render_widget(
            Paragraph::new(middle_elide(ADD_MEMBER_LABEL, footer.width as usize))
                .style(Style::default().fg(p.accent)),
            footer,
        );
    }
}

/// The composer: the chat overlay's input control, framed like every other
/// panel (`render_panel_shell`), with its title and a live character count
/// drawn on the top border. Both are derived from existing state at render
/// time — the counter is never stored.
fn render_input(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let p = &app.palette;
    let Some(inner) = render_column_frame(frame, area, COMPOSER_TITLE, p.accent, p.panel_bg) else {
        return;
    };

    // The counter shares the top border row with the title, inside the
    // corners — the title itself is painted by `render_column_frame`.
    let counter = format!("{} ", app.chat.input.chars().count());
    let counter_width = counter.chars().count() as u16;
    frame.render_widget(
        Paragraph::new(counter).style(Style::default().fg(p.overlay0)),
        Rect::new(
            area.x + area.width.saturating_sub(1 + counter_width),
            area.y,
            counter_width,
            1,
        ),
    );

    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let status = app.chat.status.as_deref().unwrap_or("");
    let prompt_width = 2;
    let input_width = inner.width.saturating_sub(prompt_width as u16 + 1) as usize;
    let mut spans = vec![
        Span::styled(
            "> ",
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            truncate_input(&app.chat.input, input_width),
            Style::default().fg(p.text),
        ),
    ];
    if !status.is_empty() {
        spans.push(Span::styled(
            format!("  {status}"),
            Style::default().fg(p.red),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn truncate_input(input: &str, max_width: usize) -> String {
    if display_width(input) <= max_width {
        return input.to_string();
    }
    // Show the tail so the newest typing stays visible.
    let mut start = input.len();
    for (idx, _) in input.char_indices().rev() {
        let width = display_width(&input[idx..]);
        if width <= max_width {
            start = idx;
        } else {
            break;
        }
    }
    input[start..].to_string()
}

fn short_time(ts: &str) -> &str {
    // RFC 3339 like 2026-08-15T15:31:02Z -> "15:31"; falls back to the raw
    // string when the format is unexpected (`.get` is boundary-safe).
    ts.split('T').nth(1).and_then(|t| t.get(0..5)).unwrap_or("")
}

/// Month abbreviations for [`short_date`] — no chrono dependency for a
/// three-letter lookup.
const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// RFC 3339 date -> block-header date, e.g. `2026-08-29T...` -> `"29-Aug"`.
/// Falls back to an empty string on an unexpected format, same contract as
/// [`short_time`].
fn short_date(ts: &str) -> String {
    let mut parts = ts.split('T').next().unwrap_or("").splitn(3, '-');
    let _year = parts.next();
    let month = parts.next().and_then(|m| m.parse::<usize>().ok());
    let day = parts.next();
    match (day, month.filter(|m| (1..=12).contains(m))) {
        (Some(day), Some(month)) => format!("{day}-{}", MONTH_ABBR[month - 1]),
        _ => String::new(),
    }
}

/// Max timeline rows one message may occupy before it collapses. A
/// ~2k-character agent post wraps to ~30 rows at typical widths and buries
/// the rest of the room; 8 rows reads about a paragraph of context while
/// leaving the timeline usable for everyone else. The one expanded message
/// renders in full (twitch-tui-style explicit `… +N lines` marker).
pub(crate) const MAX_MESSAGE_LINES: usize = 8;

/// Wrapped display lines for the message timeline. Shared by render and the
/// AppState scroll math so both agree on line counts. Styling only — the
/// wrap math is width-driven, so line counts are style-independent (the
/// count helper passes a palette it never reads through).
///
/// Sender classification per the channel contract: `from_kind == Human`
/// lines render their sender in the accent color (the composer prompt's
/// treatment), while `to_human` lines get the accent band used by the
/// selected channel row — an agent addressing the human seat must stay
/// visible while scrolling.
pub(crate) fn chat_display_lines(
    chat: &ChatViewState,
    p: &Palette,
    width: u16,
) -> Vec<Line<'static>> {
    chat_display_lines_indexed(chat, p, width)
        .into_iter()
        .map(|(_, line)| line)
        .collect()
}

pub(crate) fn chat_display_line_count(chat: &ChatViewState, width: u16) -> usize {
    chat_display_lines_indexed(chat, &Palette::catppuccin(), width).len()
}

/// Which message a wrapped display line belongs to — the input layer's
/// click-to-expand must land on the same message render drew. A collapsed
/// message's `… +N lines` marker maps to that message, so clicking the
/// marker expands it too.
pub(crate) fn chat_message_index_at_line(
    chat: &ChatViewState,
    width: u16,
    line: usize,
) -> Option<usize> {
    chat_display_lines_indexed(chat, &Palette::catppuccin(), width)
        .get(line)
        .map(|(idx, _)| *idx)
}

/// `chat_display_lines` with each line tagged by its owning message index.
/// Render, scroll math, and hit-testing all run through this one function,
/// so the three can never disagree about where a message starts or how
/// tall the timeline is.
///
/// Messages are grouped by consecutive `from_name` (chronology is never
/// reordered — "consecutive" means adjacent in `chat.messages`). Each
/// block opens with a `<label> · DD-Mon` header line, tagged with the
/// block's first message index; a blank separator line precedes every
/// header but the first, tagged with the *preceding* message's index so a
/// collapsed message's marker and the separator above the next block both
/// keep mapping to a real message for click-to-expand.
fn chat_display_lines_indexed(
    chat: &ChatViewState,
    p: &Palette,
    width: u16,
) -> Vec<(usize, Line<'static>)> {
    let width = width.max(1) as usize;
    let text_width = width.saturating_sub(MESSAGE_GUTTER).max(1);
    let indent = " ".repeat(MESSAGE_GUTTER);
    let mut lines = Vec::new();
    let mut prev_sender: Option<&str> = None;
    for (message_idx, message) in chat.messages.iter().enumerate() {
        if prev_sender != Some(message.from_name.as_str()) {
            if message_idx > 0 {
                lines.push((message_idx - 1, Line::default()));
            }
            let header = format!("{} · {}", message.from_name, short_date(&message.ts));
            lines.push((
                message_idx,
                Line::from(Span::styled(
                    header,
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                )),
            ));
        }
        prev_sender = Some(message.from_name.as_str());

        let time = short_time(&message.ts).to_string();
        // Destination token: a short inverted badge for the human seat
        // (bg-styled on its own span only — the line itself never carries
        // a bg), or the resolved pane name in dim for a targeted pane.
        // `to_names` is populated at data-refresh time (`App::pane_display_name`
        // via `App::cache_chat_to_name`); an unresolved pane falls back to
        // its raw id rather than a bare glyph.
        let dest: Option<(String, Style)> = if message.to_human {
            Some((
                "→você".to_string(),
                Style::default()
                    .bg(p.accent)
                    .fg(p.panel_bg)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            message.to_pane.as_ref().map(|pane| {
                let name = chat
                    .to_names
                    .get(pane)
                    .cloned()
                    .unwrap_or_else(|| pane.clone());
                (format!("→{name}"), Style::default().fg(p.overlay0))
            })
        };

        // The destination token rides before the text on the first line:
        // its columns come out of that line's wrap budget, or a full first
        // line plus the token overflows the panel and the tail clips.
        let token_cols = dest
            .as_ref()
            .map(|(token, _)| display_width(token) + 1)
            .unwrap_or(0);
        let first_width = text_width.saturating_sub(token_cols).max(1);
        let wrapped = wrap_width(&message.text, first_width, text_width);
        let total = wrapped.len();
        let expanded = chat.expanded_message == Some(message_idx);
        let shown = if expanded { total } else { MAX_MESSAGE_LINES };
        for (wrapped_idx, chunk) in wrapped.into_iter().take(shown).enumerate() {
            let mut spans = Vec::new();
            if wrapped_idx == 0 {
                spans.push(Span::raw(" ".repeat(MESSAGE_INDENT)));
                spans.push(Span::styled(time.clone(), Style::default().fg(p.overlay0)));
                spans.push(Span::raw(" ".repeat(TIME_GAP)));
                if let Some((token, style)) = &dest {
                    spans.push(Span::styled(token.clone(), *style));
                    spans.push(Span::raw(" "));
                }
            } else {
                spans.push(Span::raw(indent.clone()));
            }
            spans.push(Span::raw(chunk));
            lines.push((message_idx, Line::from(spans)));
        }
        // Real wrapped-line total minus what was shown — never estimated
        // from character count, which word boundaries routinely contradict.
        let hidden = total.saturating_sub(shown);
        if hidden > 0 {
            let spans = vec![
                Span::raw(indent.clone()),
                Span::styled(
                    format!("… +{hidden} lines"),
                    Style::default().fg(p.overlay0),
                ),
            ];
            lines.push((message_idx, Line::from(spans)));
        }
    }
    lines
}

/// Greedy word wrap on display width, breaking over-long words. The first
/// line wraps to `first_max`, every later line to `max_width`: an inline
/// destination token rides before the text on the first line and must
/// reserve its columns there, or the panel clips the line's tail.
fn wrap_width(text: &str, first_max: usize, max_width: usize) -> Vec<String> {
    let budget = |on_first: bool| {
        if on_first {
            first_max.max(1)
        } else {
            max_width.max(1)
        }
    };
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split(' ') {
        let word_width = display_width(word).max(1);
        if !current.is_empty() && current_width + 1 + word_width > budget(lines.is_empty()) {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if current_width + word_width > budget(lines.is_empty()) {
            // Single word longer than the line: hard-break it.
            for ch in word.chars() {
                let ch_width = display_width(&ch.to_string());
                if current_width + ch_width > budget(lines.is_empty()) {
                    lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                current.push(ch);
                current_width += ch_width;
            }
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
        }
    }
    lines.push(current);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::ChannelSenderKind;
    use crate::app::state::AppState;
    use ratatui::{backend::TestBackend, Terminal};

    fn agent_message(text: &str) -> crate::api::schema::ChannelMessage {
        crate::api::schema::ChannelMessage {
            ts: "2026-08-15T15:31:02Z".into(),
            seq: 1,
            from_pane: "w1:p1".into(),
            from_name: "builder".into(),
            from_kind: ChannelSenderKind::Agent,
            text: text.into(),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        }
    }

    /// Like `agent_message`, with an arbitrary `from_name` — the block
    /// grouping tests need to control sender identity directly.
    fn named_message(name: &str, text: &str) -> crate::api::schema::ChannelMessage {
        crate::api::schema::ChannelMessage {
            from_name: name.into(),
            ..agent_message(text)
        }
    }

    fn to_human_message(text: &str) -> crate::api::schema::ChannelMessage {
        crate::api::schema::ChannelMessage {
            to_human: true,
            ..agent_message(text)
        }
    }

    fn to_pane_message(pane: &str, text: &str) -> crate::api::schema::ChannelMessage {
        crate::api::schema::ChannelMessage {
            to_pane: Some(pane.into()),
            ..agent_message(text)
        }
    }

    /// Header lines are the only lines rendered as one bare span whose text
    /// contains the `<label> · DD-Mon` separator — used to locate them
    /// without depending on styling internals.
    fn header_labels(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .filter(|line| line.spans.len() == 1 && line.spans[0].content.contains(" · "))
            .map(|line| {
                line.spans[0]
                    .content
                    .split(" · ")
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn consecutive_same_sender_messages_share_one_header() {
        let mut state = AppState::test_new();
        state.chat.messages = vec![
            named_message("a", "one"),
            named_message("b", "two"),
            named_message("a", "three"),
            named_message("a", "four"),
            named_message("c", "five"),
        ];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);

        // a,b,a,a,c -> 4 blocks (a,b,a,c); the a,a run shares one header.
        assert_eq!(header_labels(&lines), vec!["a", "b", "a", "c"]);

        // Every header is styled accent+bold, uniformly — sender identity
        // no longer carries a per-nick colour (that lived in the deleted
        // `sender_color` hash-wheel).
        for line in &lines {
            if line.spans.len() == 1 && line.spans[0].content.contains(" · ") {
                assert_eq!(line.spans[0].style.fg, Some(state.palette.accent));
                assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
            }
        }

        // One blank separator line precedes every header but the first:
        // 4 blocks -> 3 separators.
        let blanks = lines.iter().filter(|line| line.spans.is_empty()).count();
        assert_eq!(
            blanks, 3,
            "a blank line separates every block but the first"
        );
    }

    #[test]
    fn header_carries_the_block_date() {
        let mut state = AppState::test_new();
        state.chat.messages = vec![agent_message("hi")];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);

        assert_eq!(lines[0].spans[0].content, "builder · 15-Aug");
    }

    #[test]
    fn first_line_carries_time_continuation_lines_carry_only_the_gutter_indent() {
        let mut state = AppState::test_new();
        let text = long_agent_text();
        state.chat.messages = vec![agent_message(&text)];

        // Narrow enough that the fixture wraps to several lines well under
        // the clamp, so line 2 is a genuine continuation, not the marker.
        let lines = chat_display_lines(&state.chat, &state.palette, 40);

        // Line 0 is the block header; line 1 is the message's own first
        // content line: 2-col indent, dim "HH:MM", 2-col gap, then text.
        let first = &lines[1];
        assert_eq!(first.spans[0].content, " ".repeat(MESSAGE_INDENT));
        assert_eq!(first.spans[1].content, "15:31");
        assert_eq!(first.spans[1].style.fg, Some(state.palette.overlay0));
        assert_eq!(first.spans[2].content, " ".repeat(TIME_GAP));

        // Line 2 is a wrapped continuation: only the 9-col gutter indent,
        // no timestamp, then text — exactly two spans.
        let cont = &lines[2];
        assert_eq!(
            cont.spans.len(),
            2,
            "continuation line is indent + text only"
        );
        assert_eq!(cont.spans[0].content, " ".repeat(MESSAGE_GUTTER));
    }

    #[test]
    fn to_pane_destination_resolves_through_the_to_names_cache() {
        let mut state = AppState::test_new();
        state
            .chat
            .to_names
            .insert("w1:p2".to_string(), "reviewer".to_string());
        state.chat.messages = vec![to_pane_message("w1:p2", "ping")];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);
        let content: String = lines[1]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();

        assert!(
            content.contains("→reviewer"),
            "resolves through the cache: {content}"
        );
        assert!(
            !content.contains("w1:p2"),
            "the raw pane id must not leak once resolved"
        );
        assert!(!content.contains('›'), "no bare glyph destination marker");
    }

    #[test]
    fn unresolved_to_pane_falls_back_to_the_raw_pane_id() {
        let mut state = AppState::test_new();
        state.chat.messages = vec![to_pane_message("w9:p9", "ping")];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);
        let content: String = lines[1]
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();

        assert!(
            content.contains("→w9:p9"),
            "an unresolved pane id falls back to itself, never a bare glyph: {content}"
        );
    }

    #[test]
    fn to_human_badge_is_token_scoped_not_a_full_line_band() {
        let mut state = AppState::test_new();
        state.chat.messages = vec![to_human_message("ship it")];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);
        let content_line = &lines[1];

        // The line itself carries no background — only the badge span does.
        assert_eq!(content_line.style.bg, None, "the line carries no bg");
        let badge = content_line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "→você")
            .expect("destination badge span present");
        assert_eq!(badge.style.bg, Some(state.palette.accent));
        assert_eq!(badge.style.fg, Some(state.palette.panel_bg));
        assert!(badge.style.add_modifier.contains(Modifier::BOLD));
        for span in &content_line.spans {
            if span.content.as_ref() != "→você" {
                assert_eq!(span.style.bg, None, "only the badge span carries bg");
            }
        }
    }

    #[test]
    fn destination_token_reserves_its_columns_on_the_first_line() {
        // The token is content on the first line, so the wrap budget
        // reserves its width there: a full first line plus the token must
        // still fit the panel. The wrapped total may legitimately move —
        // render and scroll math share this same wrap, so they stay in
        // agreement either way.
        let mut state = AppState::test_new();
        let text = "same text rendered twice, once plain and once addressed to the human seat";
        state.chat.messages = vec![to_human_message(text)];
        let width = 50u16;
        let lines = chat_display_lines(&state.chat, &state.palette, width);

        for line in &lines {
            let used: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
            assert!(
                used <= width as usize,
                "line overflows the {width}-col panel: {used} cols"
            );
        }
        // And the reservation is real: the same text wraps narrower on its
        // first line when a token rides before it (line 0 is the header).
        state.chat.messages = vec![agent_message(text)];
        let plain_first = chat_display_lines(&state.chat, &state.palette, width)[1]
            .spans
            .last()
            .map(|s| display_width(&s.content))
            .unwrap_or(0);
        let tokened_first = lines[1]
            .spans
            .last()
            .map(|s| display_width(&s.content))
            .unwrap_or(0);
        assert!(
            tokened_first < plain_first,
            "first line must wrap narrower with a token ({tokened_first} vs {plain_first})"
        );
    }

    /// ~2k chars of irregular words — long enough to wrap far past the
    /// clamp at any sane width, and irregular enough that a character-count
    /// estimate of the wrapped total can never be trusted to match the
    /// greedy wrap's real answer.
    fn long_agent_text() -> String {
        let words = [
            "lorem",
            "ipsum",
            "dolor",
            "sit",
            "amet",
            "consectetur",
            "adipiscing",
            "elit",
            "sed",
            "do",
            "eiusmod",
            "tempor",
            "incididunt",
            "ut",
            "labore",
            "et",
            "dolore",
            "magna",
            "aliqua",
        ];
        let mut text = String::new();
        let mut i = 0usize;
        while text.len() < 2000 {
            text.push_str(words[i % words.len()]);
            text.push(' ');
            i += 1;
        }
        text
    }

    /// The display width a message body gets inside an 80-column timeline.
    fn text_width_at(width: usize) -> usize {
        width.saturating_sub(MESSAGE_GUTTER).max(1)
    }

    #[test]
    fn long_message_clamps_to_max_lines_with_real_hidden_count() {
        let mut state = AppState::test_new();
        let text = long_agent_text();
        state.chat.messages = vec![agent_message(&text)];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);

        // The truth the marker must report: the production wrap of the same
        // text at the same width the timeline used.
        let total = wrap_width(&text, text_width_at(80), text_width_at(80)).len();
        assert!(
            total > MAX_MESSAGE_LINES + 5,
            "fixture must truly overflow the clamp (wraps to {total})"
        );

        // Header line, then exactly the clamp in content lines, plus one
        // marker line — no more, no less.
        assert_eq!(
            lines.len(),
            1 + MAX_MESSAGE_LINES + 1,
            "header + {MAX_MESSAGE_LINES} content lines + 1 marker"
        );

        // The marker sits on the last line and carries the REAL hidden
        // count: wrapped total minus shown, never chars-estimated.
        let marker = lines[1 + MAX_MESSAGE_LINES]
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect::<String>();
        assert_eq!(
            marker.trim(),
            format!("… +{} lines", total - MAX_MESSAGE_LINES),
            "marker count is the true wrapped-line remainder"
        );

        // The shown content is a prefix of the full wrap: the first
        // content line still carries the timestamp, so a collapsed message
        // remains attributable.
        assert_eq!(
            lines[1].spans[1].content, "15:31",
            "clamped first content line keeps the timestamp"
        );
    }

    #[test]
    fn the_expanded_message_renders_in_full_with_no_marker() {
        let mut state = AppState::test_new();
        let text = long_agent_text();
        state.chat.messages = vec![agent_message(&text)];
        state.chat.expanded_message = Some(0);

        let lines = chat_display_lines(&state.chat, &state.palette, 80);
        let total = wrap_width(&text, text_width_at(80), text_width_at(80)).len();

        assert_eq!(
            lines.len(),
            1 + total,
            "the header plus every wrapped line, no marker"
        );
    }

    #[test]
    fn the_marker_line_maps_back_to_its_own_message_for_clicks() {
        let mut state = AppState::test_new();
        let text = long_agent_text();
        state.chat.messages = vec![agent_message("first"), agent_message(&text)];

        // Both messages are "builder" — one shared header at line 0. Line 1
        // is "first"'s own content line; the long message's clamped content
        // starts at line 2, so its marker (collapsed) sits at 2 + MAX.
        let width = 80u16;
        assert_eq!(chat_message_index_at_line(&state.chat, width, 0), Some(0));
        assert_eq!(chat_message_index_at_line(&state.chat, width, 1), Some(0));
        assert_eq!(
            chat_message_index_at_line(&state.chat, width, 2 + MAX_MESSAGE_LINES),
            Some(1),
            "the marker row belongs to the message it collapses"
        );
    }

    // ---- composer frame (bora-7c5.2) -------------------------------------

    fn row_text(buffer: &ratatui::buffer::Buffer, row: u16, width: u16) -> String {
        (0..width)
            .map(|x| buffer[(x, row)].symbol())
            .collect::<String>()
    }

    /// Reads `width` cells starting at `(x, y)` — the column-scoped
    /// counterpart of `row_text`, for asserting a single panel's border or
    /// content without hand-slicing the full terminal row.
    fn strip_text(buffer: &ratatui::buffer::Buffer, x: u16, y: u16, width: u16) -> String {
        (x..x + width)
            .map(|col| buffer[(col, y)].symbol())
            .collect::<String>()
    }

    /// Chat state at a given terminal size, laid out like the mouse tests:
    /// 26-col sidebar, terminal beside it.
    fn chat_state_at(width: u16, height: u16) -> AppState {
        let mut state = AppState::test_new();
        state.view.sidebar_rect = Rect::new(0, 0, 26, height);
        state.view.terminal_area = Rect::new(26, 0, width.saturating_sub(26), height);
        state
    }

    #[test]
    fn composer_renders_frame_title_and_counter() {
        // 106x20 -> popup (3,1,100,18) -> inner (4,2,98,16); the composer
        // owns the bottom three inner rows (15..17) at cols 4..101. Every
        // row is asserted in full, so a missing border, title, or counter
        // cell anywhere in it fails.
        let mut state = chat_state_at(106, 20);
        let mut terminal = Terminal::new(TestBackend::new(106, 20)).expect("test terminal");
        terminal
            .draw(|frame| render_chat_overlay(&state, frame))
            .expect("chat overlay renders");

        // Empty draft -> the counter reads 0, right-aligned inside the
        // composer's top border (ends one col before the corner).
        assert_eq!(
            row_text(terminal.backend().buffer(), 15, 106),
            format!("   │┌ [ Chat ] {}0 ┐│   ", "─".repeat(84)),
            "top border: corner, title, dashes, counter, corner"
        );
        assert_eq!(
            row_text(terminal.backend().buffer(), 16, 106),
            format!("   ││> {}││   ", " ".repeat(94)),
            "input row: prompt, empty draft, padding, both frames"
        );
        assert_eq!(
            row_text(terminal.backend().buffer(), 17, 106),
            format!("   │└{}┘│   ", "─".repeat(96)),
            "bottom border of the composer frame"
        );

        // The typed draft itself renders inside the frame.
        state.chat.input = "hi".into();
        terminal
            .draw(|frame| render_chat_overlay(&state, frame))
            .expect("chat overlay renders");
        assert_eq!(
            row_text(terminal.backend().buffer(), 16, 106),
            format!("   ││> hi{}││   ", " ".repeat(92)),
            "the draft renders inside the composer frame"
        );
    }

    #[test]
    fn composer_counter_tracks_the_draft_length() {
        let mut state = chat_state_at(106, 20);
        let mut terminal = Terminal::new(TestBackend::new(106, 20)).expect("test terminal");

        let mut border_row = |input: &str| -> String {
            state.chat.input = input.into();
            terminal
                .draw(|frame| render_chat_overlay(&state, frame))
                .expect("chat overlay renders");
            row_text(terminal.backend().buffer(), 15, 106)
        };

        // Two drafts, two counters, both asserted as full rows — the dash
        // run shrinks as the counter widens, so a stale counter cannot pass.
        assert_eq!(
            border_row("hi"),
            format!("   │┌ [ Chat ] {}2 ┐│   ", "─".repeat(84))
        );
        assert_eq!(
            border_row("hello there"),
            format!("   │┌ [ Chat ] {}11 ┐│   ", "─".repeat(83))
        );
    }

    #[test]
    fn chat_column_rects_stop_where_the_composer_frame_begins() {
        // Lockstep: the composer frame consumes the bottom three inner rows,
        // so every chat column's content rect must end exactly one row above
        // it — that one row is the column's own bottom border (bora-7c5.3).
        // If one column's border-shrink math goes stale while its siblings
        // pick up a fix, the column draws under the frame (or a gap opens).
        for (width, height) in [(106u16, 20u16), (60, 24), (36, 20), (30, 8)] {
            let state = chat_state_at(width, height);
            let inner = state.chat_inner_rect();
            let input = state.chat_input_rect();

            assert_eq!(
                input.height,
                inner.height.min(3),
                "composer owns three rows at {width}x{height}"
            );
            assert_eq!(
                input.y + input.height,
                inner.y + inner.height,
                "frame flush with the inner bottom at {width}x{height}"
            );
            assert_eq!(input.x, inner.x, "full inner width");
            assert_eq!(input.width, inner.width, "full inner width");

            for (name, column) in [
                ("channel", state.chat_channel_list_rect()),
                ("members", state.chat_members_rect()),
                ("messages", state.chat_messages_rect()),
            ] {
                if column.height == 0 {
                    continue;
                }
                assert_eq!(
                    column.y + column.height + 1,
                    input.y,
                    "{name} column content plus its own bottom border ends where the composer begins at {width}x{height}"
                );
            }
            // Header + divider sit directly above the timeline body.
            let messages = state.chat_messages_rect();
            let header = state.chat_header_rect();
            if header.height > 0 {
                assert_eq!(
                    header.y + header.height,
                    messages.y,
                    "header tiles onto the timeline at {width}x{height}"
                );
            }
        }
    }

    // ---- column borders and titles (bora-7c5.3) --------------------------

    #[test]
    fn chat_columns_render_bordered_titled_panels() {
        // 106x20 -> popup (3,1,100,18) -> inner (4,2,98,16); with both side
        // columns clear of their visibility thresholds, the three panels
        // land at fixed, hand-verified positions that exactly tile the
        // inner width: channel (4,2,19,13), messages (23,2,60,13), members
        // (83,2,19,13) — replacing the old single-character separator with
        // a border and a bracketed title per column.
        let state = chat_state_at(106, 20);
        let mut terminal = Terminal::new(TestBackend::new(106, 20)).expect("test terminal");
        terminal
            .draw(|frame| render_chat_overlay(&state, frame))
            .expect("chat overlay renders");

        let channel_panel = state.chat_channel_list_panel_rect();
        let messages_panel = state.chat_messages_panel_rect();
        let members_panel = state.chat_members_panel_rect();
        assert_eq!(channel_panel, Rect::new(4, 2, 19, 13));
        assert_eq!(messages_panel, Rect::new(23, 2, 60, 13));
        assert_eq!(members_panel, Rect::new(83, 2, 19, 13));
        assert_eq!(
            channel_panel.x + channel_panel.width,
            messages_panel.x,
            "channel's right border and the timeline's left border sit in adjacent columns, no gap"
        );
        assert_eq!(
            messages_panel.x + messages_panel.width,
            members_panel.x,
            "the timeline's right border and members' left border sit in adjacent columns, no gap"
        );

        let buffer = terminal.backend().buffer();

        // Top border: corner, bracketed caption, dash run, corner — one per
        // column, each with its own title.
        assert_eq!(
            strip_text(
                buffer,
                channel_panel.x,
                channel_panel.y,
                channel_panel.width
            ),
            format!("┌ [ Channels ] {}┐", "─".repeat(3)),
            "channel column top border and title"
        );
        assert_eq!(
            strip_text(
                buffer,
                messages_panel.x,
                messages_panel.y,
                messages_panel.width
            ),
            format!("┌ [ Messages ] {}┐", "─".repeat(44)),
            "timeline column top border and title"
        );
        assert_eq!(
            strip_text(
                buffer,
                members_panel.x,
                members_panel.y,
                members_panel.width
            ),
            format!("┌ [ Members ] {}┐", "─".repeat(4)),
            "members column top border and title"
        );

        // Bottom border: plain frame, no caption, flush with the composer.
        let bottom_row = channel_panel.y + channel_panel.height - 1;
        assert_eq!(
            strip_text(buffer, channel_panel.x, bottom_row, channel_panel.width),
            format!("└{}┘", "─".repeat(17)),
            "channel column bottom border carries no caption"
        );

        // A content row: vertical bars on both edges of every column, with
        // no gap between the channel/timeline seam.
        let content_row = channel_panel.y + 1;
        assert_eq!(strip_text(buffer, channel_panel.x, content_row, 1), "│");
        assert_eq!(
            strip_text(
                buffer,
                channel_panel.x + channel_panel.width - 1,
                content_row,
                1
            ),
            "│",
            "channel's own right border"
        );
        assert_eq!(
            strip_text(buffer, messages_panel.x, content_row, 1),
            "│",
            "the timeline's own left border, immediately after channel's right border"
        );
    }

    #[test]
    fn members_column_nick_gets_ellipsis_after_the_border_shrinks_its_budget() {
        // 106x20 -> members panel (83,2,19,13) -> content (84,3,17,11): the
        // border eats 2 of the outer 19 columns, so the name/status
        // truncation must budget against 17, not 19. If that budget were
        // forgotten (truncation still sized for the unbordered column),
        // this 26-char nick would either overflow past the border or get
        // ratatui-hard-clipped with no ellipsis, and the row below would
        // stop matching.
        let mut state = chat_state_at(106, 20);
        state.chat.members = vec![crate::api::schema::ChannelMember {
            pane_id: "w1:p1".into(),
            name: Some("abcdefghijklmnopqrstuvwxyz".into()),
            agent_status: Some(AgentStatus::Idle),
            source: crate::api::schema::ChannelMemberSource::Workspace,
            unread: 0,
        }];
        let mut terminal = Terminal::new(TestBackend::new(106, 20)).expect("test terminal");
        terminal
            .draw(|frame| render_chat_overlay(&state, frame))
            .expect("chat overlay renders");

        let members = state.chat_members_rect();
        assert_eq!(
            members,
            Rect::new(84, 3, 17, 11),
            "content rect is the members panel minus its own border"
        );

        let buffer = terminal.backend().buffer();
        let row = strip_text(buffer, members.x, members.y + 1, members.width);
        assert_eq!(
            row, "○ abc…wxyz idle ×",
            "the 26-char nick is elided to fit the border-shrunk 17-column row"
        );
    }

    #[test]
    fn chat_column_content_positions_track_the_panel_across_widths() {
        // Every column's content starts exactly one cell inside its own
        // panel's border, on both axes. If one column's panel-to-content
        // arithmetic goes stale while its siblings pick up a fix, the
        // border and the drawn content drift apart — this renders and
        // checks both at once, at several widths where all three columns
        // are visible.
        for (width, height) in [(106u16, 20u16), (140, 24)] {
            let state = chat_state_at(width, height);
            let mut terminal =
                Terminal::new(TestBackend::new(width, height)).expect("test terminal");
            terminal
                .draw(|frame| render_chat_overlay(&state, frame))
                .expect("chat overlay renders");

            let channel_panel = state.chat_channel_list_panel_rect();
            let messages_panel = state.chat_messages_panel_rect();
            let members_panel = state.chat_members_panel_rect();
            assert!(
                channel_panel.width > 0 && members_panel.width > 0,
                "both side columns are visible at {width}x{height}"
            );

            let channel_content = state.chat_channel_list_rect();
            let members_content = state.chat_members_rect();
            let header = state.chat_header_rect();

            for (name, panel, content) in [
                ("channel", channel_panel, channel_content),
                ("members", members_panel, members_content),
                ("messages header", messages_panel, header),
            ] {
                assert_eq!(
                    (content.x, content.y),
                    (panel.x + 1, panel.y + 1),
                    "{name} content starts one cell inside its own border at {width}x{height}"
                );
                assert_eq!(
                    content.width,
                    panel.width - 2,
                    "{name} content is the panel minus its border at {width}x{height}"
                );
            }

            let buffer = terminal.backend().buffer();
            assert_eq!(
                buffer[(channel_content.x, channel_content.y)].symbol(),
                "n",
                "the channel column's empty-state text starts at the content origin, not the border, at {width}x{height}"
            );
            assert_eq!(
                buffer[(members_content.x, members_content.y)].symbol(),
                "0",
                "the members header starts at the content origin at {width}x{height}"
            );
        }
    }

    // ---- unread marker (bora-7c5.4) ---------------------------------

    fn channel_fixture(name: &str, unread: u64) -> crate::api::schema::ChannelSummary {
        crate::api::schema::ChannelSummary {
            name: name.into(),
            pane_count: 1,
            agent_count: 1,
            last_message_seq: 42,
            last_message_ts: Some("2026-08-15T10:00:00Z".into()),
            unread,
            member_status_counts: Default::default(),
        }
    }

    #[test]
    fn channel_with_unread_shows_marker_read_channel_shows_nothing() {
        // Two channels, identical name/detail, differing only in `unread`:
        // the rendered rows must differ only by the trailing marker span.
        // The read row carries no residue of it — no empty bracket, no
        // "0", nothing a reader has to learn to ignore.
        let mut state = chat_state_at(106, 20);
        state.chat.channels = vec![channel_fixture("#a", 3), channel_fixture("#a", 0)];
        state.chat.selected = 99; // neither row selected

        let mut terminal = Terminal::new(TestBackend::new(106, 20)).expect("test terminal");
        terminal
            .draw(|frame| render_chat_overlay(&state, frame))
            .expect("chat overlay renders");

        let content = state.chat_channel_list_rect();
        let buffer = terminal.backend().buffer();
        let unread_row = strip_text(buffer, content.x, content.y, content.width);
        let read_row = strip_text(buffer, content.x, content.y + 1, content.width);

        assert_eq!(
            unread_row, " #a  1·1 10:00 3●",
            "unread channel: label then a fixed-width unread marker"
        );
        assert_eq!(
            read_row, " #a  1·1 10:00   ",
            "read channel: identical label, no marker, no residue in its place"
        );
        assert_eq!(
            unread_row,
            format!("{} 3●", read_row.trim_end()),
            "the two rows differ only by the trailing marker"
        );
    }

    #[test]
    fn unread_marker_survives_the_narrowest_bordered_channel_column() {
        // 46-wide terminal -> onboarding area union width 46 -> popup
        // width 42 -> inner width 40, the exact threshold below which the
        // channel column hides entirely (`chat_channel_list_outer_width`).
        // At 40 the column is 12 wide including its own border, 10 wide
        // inside it — the narrowest content width the chat view ever
        // shows. The channel name/detail must lose the eliding race
        // against the marker, not the other way around: the marker must
        // come through intact rather than get ratatui-hard-clipped.
        let mut state = chat_state_at(46, 20);
        assert_eq!(
            state.chat_inner_rect().width,
            40,
            "fixture must sit exactly at the column-visibility threshold"
        );
        state.chat.channels = vec![channel_fixture("#g", 3)];
        state.chat.selected = 99; // not selected

        let mut terminal = Terminal::new(TestBackend::new(46, 20)).expect("test terminal");
        terminal
            .draw(|frame| render_chat_overlay(&state, frame))
            .expect("chat overlay renders");

        let content = state.chat_channel_list_rect();
        assert_eq!(content.width, 10, "narrowest channel column content width");
        let buffer = terminal.backend().buffer();
        let row = strip_text(buffer, content.x, content.y, content.width);

        assert_eq!(
            row, " #g…:00 3●",
            "name/detail elides so the full, unclipped marker fits at the narrowest width"
        );
    }
}
