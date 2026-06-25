use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    Frame,
};

use crate::app::state::{AppState, Mode, RightPanelTab};

/// Render the right panel (Changes / Checks tabs).
pub(super) fn render_right_panel(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    let buf = frame.buffer_mut();

    // Left-edge separator — mirrors the sidebar's right-edge divider
    let sep_x = area.x;
    let is_navigating = matches!(app.mode, Mode::Navigate);
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    // Content area starts one column after separator
    let content = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };
    if content.width == 0 || content.height < 2 {
        return;
    }

    let [tab_row, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(content);

    render_tab_header(app, frame, tab_row);

    match app.right_panel_active_tab {
        RightPanelTab::Changes => render_changes_tab(app, frame, body),
        RightPanelTab::Checks => render_checks_tab(app, frame, body),
    }
    render_right_panel_toggle(app, frame, area);
}

fn render_tab_header(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let active = app.right_panel_active_tab;
    let changes_style = if active == RightPanelTab::Changes {
        Style::default().fg(p.text).bold()
    } else {
        Style::default().fg(p.subtext0)
    };
    let checks_style = if active == RightPanelTab::Checks {
        Style::default().fg(p.text).bold()
    } else {
        Style::default().fg(p.subtext0)
    };
    let line = Line::from(vec![
        Span::styled(" Changes ", changes_style),
        Span::raw("│"),
        Span::styled(" Checks ", checks_style),
    ]);
    frame.render_widget(line, area);
}

fn render_changes_tab(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let ws = app.active.and_then(|i| app.workspaces.get(i));
    let change_set = ws.and_then(|ws| ws.cached_change_set.as_ref());

    let Some(cs) = change_set else {
        frame.render_widget(
            Line::from(Span::styled(" no changes", Style::default().fg(p.subtext0))),
            area,
        );
        return;
    };

    if cs.sections.is_empty() {
        frame.render_widget(
            Line::from(Span::styled(" clean", Style::default().fg(p.subtext0))),
            area,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();
    for section in &cs.sections {
        use crate::workspace::ChangeSectionKind;
        let header = match section.kind {
            ChangeSectionKind::Unstaged => "Changes",
            ChangeSectionKind::Staged => "Staged Changes",
            ChangeSectionKind::Committed => "Committed",
        };
        lines.push(Line::from(Span::styled(
            format!(" {} ({})", header, section.files.len()),
            Style::default().fg(p.text).bold(),
        )));

        for file in &section.files {
            use crate::workspace::ChangeStatus;
            let (status_char, status_color) = match file.status {
                ChangeStatus::Modified => ('M', p.accent),
                ChangeStatus::Added => ('A', p.green),
                ChangeStatus::Deleted => ('D', p.red),
                ChangeStatus::Renamed => ('R', p.accent),
                ChangeStatus::Untracked => ('?', p.subtext0),
            };
            let delta = match (file.added, file.removed) {
                (Some(a), Some(r)) => format!(" +{a} -{r}"),
                _ => " bin".to_string(),
            };
            let filename = file.path.rsplit('/').next().unwrap_or(&file.path);
            lines.push(Line::from(vec![
                Span::styled(format!("  {status_char} "), Style::default().fg(status_color)),
                Span::styled(filename.to_string(), Style::default().fg(p.text)),
                Span::styled(delta, Style::default().fg(p.subtext0)),
            ]));
        }
    }

    let scroll = app.right_panel_scroll as usize;
    let visible = area.height as usize;
    let display_lines: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(Text::from(display_lines), area);
}

fn render_checks_tab(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let ws = app.active.and_then(|i| app.workspaces.get(i));
    let check_status = ws.and_then(|ws| ws.cached_check_status.as_ref());

    let Some(cs) = check_status else {
        frame.render_widget(
            Line::from(Span::styled(" no check data", Style::default().fg(p.subtext0))),
            area,
        );
        return;
    };

    if let Some(err) = &cs.error {
        frame.render_widget(
            Line::from(Span::styled(format!(" {err}"), Style::default().fg(p.red))),
            area,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    if let Some(pr) = &cs.pr {
        lines.push(Line::from(vec![
            Span::styled(format!(" #{} ", pr.number), Style::default().fg(p.accent).bold()),
            Span::styled(pr.title.clone(), Style::default().fg(p.text)),
        ]));
        let state_color = match pr.state.as_str() {
            "OPEN" => p.green,
            "CLOSED" => p.red,
            "MERGED" => p.accent,
            _ => p.subtext0,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", pr.state), Style::default().fg(state_color)),
            Span::styled(
                pr.mergeable.as_deref().unwrap_or("").to_string(),
                Style::default().fg(p.subtext0),
            ),
        ]));
    }

    if cs.checks.is_empty() {
        lines.push(Line::from(Span::styled(
            " no checks",
            Style::default().fg(p.subtext0),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!(" Checks ({})", cs.checks.len()),
            Style::default().fg(p.text).bold(),
        )));
        for check in &cs.checks {
            let (icon, icon_color) = match check.conclusion.as_deref() {
                Some("SUCCESS") => ("✓", p.green),
                Some("FAILURE") | Some("TIMED_OUT") | Some("CANCELLED") => ("✗", p.red),
                Some("NEUTRAL") | Some("SKIPPED") => ("−", p.subtext0),
                _ if check.status == "IN_PROGRESS" => ("◑", p.subtext0),
                _ => ("○", p.subtext0),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {icon} "), Style::default().fg(icon_color)),
                Span::styled(check.name.clone(), Style::default().fg(p.text)),
            ]));
        }
    }

    frame.render_widget(Text::from(lines), area);
}

// ── Toggle button ────────────────────────────────────────────────────────────

/// Toggle icon at the bottom-left of the expanded right panel (just inside the separator).
fn render_right_panel_toggle(app: &AppState, frame: &mut Frame, area: Rect) {
    let toggle_area = expanded_right_panel_toggle_rect(area);
    if toggle_area == Rect::default() {
        return;
    }
    let icon_style = Style::default().fg(app.palette.overlay0);
    frame.render_widget(
        ratatui::widgets::Paragraph::new(Span::styled("»", icon_style)),
        toggle_area,
    );
}

/// Render a collapsed-state toggle hint at the right edge of the terminal area.
/// Called from the main render path when the right panel is collapsed.
pub(super) fn render_right_panel_collapsed_toggle(app: &AppState, frame: &mut Frame, area: Rect) {
    let toggle_area = collapsed_right_panel_toggle_rect(area);
    if toggle_area == Rect::default() {
        return;
    }
    let icon_style = Style::default().fg(app.palette.overlay0);
    frame.render_widget(
        ratatui::widgets::Paragraph::new(Span::styled("«", icon_style)),
        toggle_area,
    );
}

/// Rect for the toggle icon when the right panel is expanded.
/// Bottom-left corner of the panel, on the separator column.
pub(crate) fn expanded_right_panel_toggle_rect(area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }
    Rect::new(area.x + 1, area.y + area.height.saturating_sub(1), 1, 1)
}

/// Rect for the toggle icon when the right panel is collapsed.
/// Bottom-right corner of the terminal area (rightmost column, bottom row).
pub(crate) fn collapsed_right_panel_toggle_rect(area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }
    Rect::new(
        area.x + area.width.saturating_sub(1),
        area.y + area.height.saturating_sub(1),
        1,
        1,
    )
}
