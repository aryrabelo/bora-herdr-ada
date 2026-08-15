//! Chat view render — senpai-style three columns inside bora's panel shell:
//! channel list | message timeline | member list, with a composer line.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
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
use crate::app::state::{AppState, ChatViewState};

const TIME_WIDTH: usize = 5; // "HH:MM"
const SENDER_WIDTH: usize = 14;
const COLUMN_GAP: usize = 1;

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
}

fn render_channel_list(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let p = &app.palette;
    for (idx, channel) in app
        .chat
        .channels
        .iter()
        .enumerate()
        .take(area.height as usize)
    {
        let selected = idx == app.chat.selected;
        let style = if selected {
            Style::default().bg(p.accent).fg(p.panel_bg)
        } else {
            Style::default().fg(panel_contrast_fg(p))
        };
        let label = middle_elide(&channel.name, area.width.saturating_sub(1) as usize);
        let row = if selected {
            format!("▐{label}")
        } else {
            format!(" {label}")
        };
        frame.render_widget(
            Paragraph::new(row).style(style),
            Rect::new(area.x, area.y + idx as u16, area.width, 1),
        );
    }
    if app.chat.channels.is_empty() {
        frame.render_widget(
            Paragraph::new("no channels").style(Style::default().fg(p.overlay0)),
            area,
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
    let lines = chat_display_lines(&app.chat, area.width);
    let start = app.chat.scroll.min(lines.len());
    let end = lines.len().min(start + area.height as usize);
    let visible: Vec<Line<'static>> = lines[start..end].to_vec();
    frame.render_widget(Paragraph::new(visible), area);
}

fn render_members(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let p = &app.palette;
    let header = format!("{} members", app.chat.members.len());
    frame.render_widget(
        Paragraph::new(header).style(Style::default().fg(p.overlay0)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    for (idx, member) in app
        .chat
        .members
        .iter()
        .enumerate()
        .take(area.height.saturating_sub(1) as usize)
    {
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
        let mut spans = vec![
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(
                middle_elide(&name, area.width.saturating_sub(2) as usize),
                Style::default().fg(match state {
                    Some(state) => state_label_color(state, true, &app.palette),
                    None => p.overlay0,
                }),
            ),
        ];
        if let Some(status) = member.agent_status {
            let label = match status {
                AgentStatus::Idle => "idle",
                AgentStatus::Working => "working",
                AgentStatus::Blocked => "blocked",
                AgentStatus::Done => "done",
                AgentStatus::Unknown => "",
            };
            spans.push(Span::styled(
                format!(" {label}"),
                Style::default().fg(p.overlay0),
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), row);
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

/// Wrapped display lines for the message timeline. Shared by render and the
/// AppState scroll math so both agree on line counts.
pub(crate) fn chat_display_lines(chat: &ChatViewState, width: u16) -> Vec<Line<'static>> {
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
        for (wrapped_idx, chunk) in wrap_width(&message.text, text_width)
            .into_iter()
            .enumerate()
        {
            let mut spans = Vec::new();
            if wrapped_idx == 0 {
                spans.push(Span::raw(format!("{time}{COLUMN_GAP}")));
                spans.push(Span::styled(
                    sender.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
            } else {
                spans.push(Span::raw(indent.clone()));
            }
            spans.push(Span::raw(chunk));
            lines.push(Line::from(spans));
        }
    }
    lines
}

pub(crate) fn chat_display_line_count(chat: &ChatViewState, width: u16) -> usize {
    chat_display_lines(chat, width).len()
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
