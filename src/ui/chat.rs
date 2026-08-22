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
use crate::api::schema::{AgentStatus, ChannelSenderKind};
use crate::app::state::{AppState, ChatPrompt, ChatViewState, Palette};

const TIME_WIDTH: usize = 5; // "HH:MM"
const SENDER_WIDTH: usize = 14;
const COLUMN_GAP: usize = 1;
/// Bottom row of the channel column: click (or Ctrl+N) to create a channel.
/// Same `+` vocabulary the sidebar uses for "new worktree" / "run command".
pub(crate) const NEW_CHANNEL_LABEL: &str = "+ new channel";

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

    let list = app.chat_channel_list_rect();
    let messages = app.chat_messages_rect();
    let members = app.chat_members_rect();
    let input = app.chat_input_rect();

    render_channel_list(app, frame, list);
    render_column_separator(frame, list, app);
    render_messages(app, frame, messages);
    if members.width > 0 {
        render_column_separator(frame, messages, app);
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
        // Unread is a separate, fixed-width span appended after the elided
        // name/detail so it's never swallowed by `middle_elide`'s "…" and
        // stays visually distinct (teal, bold) from the pane/agent counts —
        // it's rendered as its own span, not concatenated into that text.
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

fn render_column_separator(frame: &mut Frame, left: Rect, app: &AppState) {
    if left.height == 0 || left.width == 0 {
        return;
    }
    let area = Rect::new(left.x + left.width, left.y, 1, left.height);
    if area.width == 0 {
        return;
    }
    let line = "─".repeat(area.height as usize);
    for (offset, ch) in line.chars().enumerate() {
        frame.render_widget(
            Paragraph::new(ch.to_string()).style(Style::default().fg(app.palette.surface1)),
            Rect::new(area.x, area.y + offset as u16, 1, 1),
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

fn render_input(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let p = &app.palette;
    let status = app.chat.status.as_deref().unwrap_or("");
    let prompt_width = 2;
    let input_width = area.width.saturating_sub(prompt_width as u16 + 1) as usize;
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
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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

/// Deterministic per-sender colour, the way every IRC client since the
/// nineties has done it: same nick, same hue, every session, no state. Makes
/// a wall of agent traffic scannable — you find your reviewer by colour
/// before you finish reading the name. `to_human` and `Human` senders are
/// styled by the caller and never reach this.
fn sender_color(name: &str, p: &Palette) -> Color {
    // FNV-1a: tiny, stable across runs and platforms, and unlike `DefaultHasher`
    // it is specified — a hash that changed between releases would repaint
    // everyone's nick on upgrade.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    let wheel = [p.mauve, p.green, p.yellow, p.blue, p.teal, p.peach, p.red];
    wheel[hash as usize % wheel.len()]
}

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
    let width = width.max(1) as usize;
    let text_width = width
        .saturating_sub(TIME_WIDTH + SENDER_WIDTH + COLUMN_GAP * 2)
        .max(1);
    let mut lines = Vec::new();
    for message in &chat.messages {
        let time = short_time(&message.ts);
        let mut sender = middle_elide(&message.from_name, SENDER_WIDTH);
        if message.to_pane.is_some() {
            sender = format!("›{sender}");
        }
        let sender = format!("{sender:>width$}", width = SENDER_WIDTH);
        let indent = " ".repeat(TIME_WIDTH + COLUMN_GAP + SENDER_WIDTH + COLUMN_GAP);
        // The band restyles every span on the line, so the sender keeps
        // only weight there — accent-on-accent would be invisible.
        let band = message
            .to_human
            .then(|| Style::default().bg(p.accent).fg(p.panel_bg));
        let sender_style = if message.to_human {
            Style::default().add_modifier(Modifier::BOLD)
        } else if message.from_kind == ChannelSenderKind::Human {
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(sender_color(&message.from_name, p))
                .add_modifier(Modifier::BOLD)
        };
        for (wrapped_idx, chunk) in wrap_width(&message.text, text_width)
            .into_iter()
            .enumerate()
        {
            let mut spans = Vec::new();
            if wrapped_idx == 0 {
                // Literal separator, not `{COLUMN_GAP}` — that formats the
                // usize and printed a stray "1" after every timestamp
                // ("16:111" instead of "16:11 ").
                spans.push(Span::raw(format!("{time}{}", " ".repeat(COLUMN_GAP))));
                spans.push(Span::styled(sender.clone(), sender_style));
                spans.push(Span::raw(" "));
            } else {
                spans.push(Span::raw(indent.clone()));
            }
            spans.push(Span::raw(chunk));
            lines.push(match band {
                Some(style) => Line::from(spans).style(style),
                None => Line::from(spans),
            });
        }
    }
    lines
}

pub(crate) fn chat_display_line_count(chat: &ChatViewState, width: u16) -> usize {
    chat_display_lines(chat, &Palette::catppuccin(), width).len()
}

/// Greedy word wrap on display width, breaking over-long words.
fn wrap_width(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split(' ') {
        let word_width = display_width(word).max(1);
        if !current.is_empty() && current_width + 1 + word_width > max_width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if current_width + word_width > max_width {
            // Single word longer than the line: hard-break it.
            for ch in word.chars() {
                let ch_width = display_width(&ch.to_string());
                if current_width + ch_width > max_width {
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

    fn human_message(name: &str, text: &str) -> crate::api::schema::ChannelMessage {
        crate::api::schema::ChannelMessage {
            from_pane: String::new(),
            from_name: name.into(),
            from_kind: ChannelSenderKind::Human,
            to_human: false,
            ..agent_message(text)
        }
    }

    fn to_human_message(text: &str) -> crate::api::schema::ChannelMessage {
        crate::api::schema::ChannelMessage {
            to_human: true,
            ..agent_message(text)
        }
    }

    /// The sender span of the first wrapped line of `message`.
    fn sender_span<'a>(lines: &'a [Line<'static>], idx: usize) -> &'a Span<'static> {
        &lines[idx].spans[1]
    }

    #[test]
    fn timestamp_column_is_time_then_space_not_the_gap_constant() {
        let mut state = AppState::test_new();
        state.chat.messages = vec![agent_message("oi")];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);
        let time = lines[0].spans[0].content.to_string();

        // Regression: this span was `format!("{time}{COLUMN_GAP}")`, which
        // formats the usize and rendered "15:311" on every single line.
        assert_eq!(time, "15:31 ");
        assert!(
            !time.ends_with(&COLUMN_GAP.to_string()),
            "the gap constant must never be printed as a digit"
        );
    }

    #[test]
    fn sender_colour_is_stable_and_spreads_across_the_wheel() {
        let p = crate::app::state::Palette::catppuccin();

        // Same nick, same hue — every session, or the view repaints itself
        // between restarts and colour stops being a way to find anyone.
        assert_eq!(sender_color("reviewer", &p), sender_color("reviewer", &p));

        // And the wheel actually spreads: a handful of real pane names must
        // not all collapse onto one colour, which would pass a stability
        // test while being useless on screen.
        let names = ["builder", "reviewer", "scout", "w40:p1", "w42:p1", "omp"];
        let distinct: std::collections::HashSet<_> =
            names.iter().map(|n| sender_color(n, &p)).collect();
        assert!(
            distinct.len() >= 3,
            "6 senders collapsed onto {} colour(s)",
            distinct.len()
        );
    }

    #[test]
    fn human_and_agent_senders_render_distinctly() {
        let mut state = AppState::test_new();
        state.chat.messages = vec![
            agent_message("deploying now"),
            human_message("ary", "ship it"),
        ];

        let lines = chat_display_lines(&state.chat, &state.palette, 80);

        assert_eq!(lines.len(), 2, "both messages fit on one line each");
        let agent = sender_span(&lines, 0).style;
        let human = sender_span(&lines, 1).style;
        assert_eq!(
            agent.fg,
            Some(sender_color("builder", &state.palette)),
            "agent sender carries its own deterministic colour"
        );
        assert_ne!(
            agent.fg,
            Some(state.palette.accent),
            "and it is never the accent, which is reserved for the human seat"
        );
        assert_eq!(
            human.fg,
            Some(state.palette.accent),
            "human sender carries the accent color"
        );
        assert!(human.add_modifier.contains(Modifier::BOLD));
        assert_ne!(agent, human, "human vs agent lines are visually distinct");

        // The human's own line reads as them: the configured chat name is
        // the sender label, right-aligned like every sender.
        let label = sender_span(&lines, 1).content.to_string();
        assert_eq!(label, format!("{:>width$}", "ary", width = SENDER_WIDTH));
    }

    #[test]
    fn to_human_messages_get_the_highlight_band_on_every_wrapped_line() {
        let mut state = AppState::test_new();
        let long = "a to-human message long enough to wrap across several timeline rows so the band must cover continuation lines too";
        state.chat.messages = vec![agent_message("chatter"), to_human_message(long)];

        let lines = chat_display_lines(&state.chat, &state.palette, 60);

        assert_eq!(lines[0].style.bg, None, "broadcast line has no band");
        let band = Style::default()
            .bg(state.palette.accent)
            .fg(state.palette.panel_bg);
        assert!(lines.len() > 3, "the long message wraps to multiple lines");
        for line in &lines[1..] {
            assert_eq!(line.style, band, "every wrapped line carries the band");
        }
        // The band leaves the sender readable: bold survives, no accent fg.
        let sender = sender_span(&lines, 1).style;
        assert!(sender.add_modifier.contains(Modifier::BOLD));
        assert_ne!(sender.fg, Some(state.palette.accent));
    }

    #[test]
    fn to_human_flag_does_not_change_wrapped_line_count() {
        // The scroll math shares this wrap path; styling must stay
        // count-neutral or scroll offsets drift between renders.
        let mut state = AppState::test_new();
        let text = "same text rendered twice, once plain and once addressed to the human seat";
        state.chat.messages = vec![agent_message(text)];
        let plain = chat_display_line_count(&state.chat, 50);

        state.chat.messages = vec![to_human_message(text)];
        let highlighted = chat_display_line_count(&state.chat, 50);

        assert_eq!(plain, highlighted);
        assert!(plain > 1);
    }
}
