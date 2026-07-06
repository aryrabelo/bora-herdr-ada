use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span, Text},
    Frame,
};

use crate::app::state::{AppState, Mode, RightPanelTab};

/// Tab order and header labels — the single source of truth shared by
/// `render_tab_header` and `right_panel_tab_hit` so render and hit-testing
/// cannot diverge.
const RIGHT_PANEL_TABS: [(RightPanelTab, &str); 4] = [
    (RightPanelTab::Changes, " Changes "),
    (RightPanelTab::Checks, " Checks "),
    (RightPanelTab::Issues, " Issues "),
    (RightPanelTab::PullRequests, " PRs "),
];

/// Map a click column on the tab header row to the tab whose label segment
/// it hits. Divider columns between segments and columns past the last label
/// return `None`. `panel` is the full right-panel rect (label layout starts
/// one column after the left separator).
pub(crate) fn right_panel_tab_hit(col: u16, panel: Rect) -> Option<RightPanelTab> {
    if panel.width == 0 {
        return None;
    }
    let end = panel.x + panel.width;
    let mut cursor = panel.x + 1; // content starts after the separator column
    for (i, (tab, label)) in RIGHT_PANEL_TABS.iter().enumerate() {
        if i > 0 {
            if col == cursor {
                return None; // "│" divider column
            }
            cursor = cursor.saturating_add(1);
        }
        let width = label.chars().count() as u16;
        if col >= cursor && col < cursor.saturating_add(width).min(end) {
            return Some(*tab);
        }
        cursor = cursor.saturating_add(width);
    }
    None
}

/// Render the right panel (Changes / Checks / Issues tabs).
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
        RightPanelTab::Issues => render_issues_tab(app, frame, body),
        RightPanelTab::PullRequests => render_prs_tab(app, frame, body),
    }
    render_right_panel_toggle(app, frame, area);
}

fn render_tab_header(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let active = app.right_panel_active_tab;
    let mut spans: Vec<Span> = Vec::new();
    for (i, (tab, label)) in RIGHT_PANEL_TABS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("│"));
        }
        let style = if active == *tab {
            Style::default().fg(p.text).bold()
        } else {
            Style::default().fg(p.subtext0)
        };
        spans.push(Span::styled(*label, style));
    }
    frame.render_widget(Line::from(spans), area);
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
                Span::styled(
                    format!("  {status_char} "),
                    Style::default().fg(status_color),
                ),
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
            Line::from(Span::styled(
                " no check data",
                Style::default().fg(p.subtext0),
            )),
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
            Span::styled(
                format!(" #{} ", pr.number),
                Style::default().fg(p.accent).bold(),
            ),
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

fn render_issues_tab(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let dim_line = |msg: &str| {
        Line::from(Span::styled(
            msg.to_string(),
            Style::default().fg(p.subtext0),
        ))
    };

    let ws = app.active.and_then(|i| app.workspaces.get(i));
    let repo_identity = ws
        .and_then(|ws| ws.git_space())
        .map(|space| space.repo_identity.clone());
    let Some(repo_identity) = repo_identity else {
        frame.render_widget(dim_line(" not a git repository"), area);
        return;
    };

    let Some(cache) = app.repo_issues.get(&repo_identity) else {
        let msg = if app.right_panel_issues_requested
            || app.issues_fetch_in_flight.contains(&repo_identity)
        {
            " loading issues…"
        } else {
            " no issues data"
        };
        frame.render_widget(dim_line(msg), area);
        return;
    };

    if let Some(err) = &cache.error {
        frame.render_widget(
            Line::from(Span::styled(format!(" {err}"), Style::default().fg(p.red))),
            area,
        );
        return;
    }

    if cache.issues.is_empty() {
        frame.render_widget(dim_line(" no open issues"), area);
        return;
    }

    let lines: Vec<Line> = cache
        .issues
        .iter()
        .map(|issue| {
            Line::from(vec![
                Span::styled(
                    format!(" #{} ", issue.number),
                    Style::default().fg(p.accent).bold(),
                ),
                Span::styled(issue.title.clone(), Style::default().fg(p.text)),
            ])
        })
        .collect();

    let scroll = app.right_panel_scroll as usize;
    let visible = area.height as usize;
    let display_lines: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(Text::from(display_lines), area);
}

fn render_prs_tab(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let dim_line = |msg: &str| {
        Line::from(Span::styled(
            msg.to_string(),
            Style::default().fg(p.subtext0),
        ))
    };

    let ws = app.active.and_then(|i| app.workspaces.get(i));
    let repo_identity = ws
        .and_then(|ws| ws.git_space())
        .map(|space| space.repo_identity.clone());
    let Some(repo_identity) = repo_identity else {
        frame.render_widget(dim_line(" not a git repository"), area);
        return;
    };

    let Some(cache) = app.repo_open_prs.get(&repo_identity) else {
        let msg =
            if app.right_panel_prs_requested || app.prs_fetch_in_flight.contains(&repo_identity) {
                " loading pull requests…"
            } else {
                " no pull request data"
            };
        frame.render_widget(dim_line(msg), area);
        return;
    };

    if let Some(err) = &cache.error {
        frame.render_widget(
            Line::from(Span::styled(format!(" {err}"), Style::default().fg(p.red))),
            area,
        );
        return;
    }

    if cache.prs.is_empty() {
        frame.render_widget(dim_line(" no open pull requests"), area);
        return;
    }

    let lines: Vec<Line> = cache
        .prs
        .iter()
        .map(|pr| {
            let (num_style, prefix) = if pr.is_draft {
                (Style::default().fg(p.subtext0), "◌ ")
            } else {
                (Style::default().fg(p.green).bold(), "")
            };
            let title_style = if pr.is_draft {
                Style::default().fg(p.subtext0)
            } else {
                Style::default().fg(p.text)
            };
            let (mark, mark_style) = match pr.mergeable.as_deref() {
                Some("MERGEABLE") => (" ✓", Style::default().fg(p.green)),
                Some("CONFLICTING") => (" ✗", Style::default().fg(p.red)),
                _ => ("", Style::default().fg(p.subtext0)),
            };
            Line::from(vec![
                Span::styled(format!(" {prefix}#{} ", pr.number), num_style),
                Span::styled(pr.title.clone(), title_style),
                Span::styled(mark.to_string(), mark_style),
            ])
        })
        .collect();

    let scroll = app.right_panel_scroll as usize;
    let visible = area.height as usize;
    let display_lines: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();
    frame.render_widget(Text::from(display_lines), area);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::workspace::Workspace;
    use ratatui::{backend::TestBackend, Terminal};

    // ── right_panel_tab_hit ──────────────────────────────────────────────────

    #[test]
    fn tab_hit_maps_label_segments_and_dividers() {
        let panel = Rect::new(80, 0, 40, 20);
        let changes_w = " Changes ".len() as u16;
        let checks_w = " Checks ".len() as u16;
        let issues_w = " Issues ".len() as u16;
        let changes_start = panel.x + 1; // content begins after the separator
        let checks_start = changes_start + changes_w + 1;
        let issues_start = checks_start + checks_w + 1;

        // The separator column is not part of any segment.
        assert_eq!(right_panel_tab_hit(panel.x, panel), None);

        // First and last column of each label segment.
        assert_eq!(
            right_panel_tab_hit(changes_start, panel),
            Some(RightPanelTab::Changes)
        );
        assert_eq!(
            right_panel_tab_hit(changes_start + changes_w - 1, panel),
            Some(RightPanelTab::Changes)
        );
        assert_eq!(
            right_panel_tab_hit(checks_start, panel),
            Some(RightPanelTab::Checks)
        );
        assert_eq!(
            right_panel_tab_hit(checks_start + checks_w - 1, panel),
            Some(RightPanelTab::Checks)
        );
        assert_eq!(
            right_panel_tab_hit(issues_start, panel),
            Some(RightPanelTab::Issues)
        );
        assert_eq!(
            right_panel_tab_hit(issues_start + issues_w - 1, panel),
            Some(RightPanelTab::Issues)
        );

        // Divider columns between segments hit nothing.
        assert_eq!(right_panel_tab_hit(changes_start + changes_w, panel), None);
        assert_eq!(right_panel_tab_hit(checks_start + checks_w, panel), None);

        // Past the last label (still inside and outside the panel).
        assert_eq!(right_panel_tab_hit(issues_start + issues_w, panel), None);
        assert_eq!(right_panel_tab_hit(panel.x + panel.width, panel), None);
        assert_eq!(right_panel_tab_hit(u16::MAX, panel), None);
    }

    #[test]
    fn tab_hit_handles_collapsed_and_clipped_panels() {
        // Zero-width panel never hits.
        assert_eq!(right_panel_tab_hit(0, Rect::new(0, 0, 0, 20)), None);
        assert_eq!(right_panel_tab_hit(5, Rect::new(5, 0, 0, 20)), None);

        // A narrow panel clips segments at its right edge: the visible part
        // of a label still hits, columns past the edge never do.
        let narrow = Rect::new(0, 0, 12, 20);
        assert_eq!(right_panel_tab_hit(1, narrow), Some(RightPanelTab::Changes));
        assert_eq!(right_panel_tab_hit(11, narrow), Some(RightPanelTab::Checks));
        assert_eq!(right_panel_tab_hit(12, narrow), None);
        assert_eq!(right_panel_tab_hit(30, narrow), None);
    }

    // ── Issues tab render states ─────────────────────────────────────────────

    const TEST_REPO: &str = "github.com/owner/proj";

    fn issues_app() -> AppState {
        let mut app = AppState::test_new();
        let mut ws = Workspace::test_new("proj");
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: "key-p".into(),
            repo_identity: TEST_REPO.into(),
            checkout_key: "/repo/proj".into(),
            label: "proj".into(),
            repo_root: std::path::PathBuf::from("/repo/proj"),
            is_linked_worktree: false,
        });
        app.workspaces = vec![ws];
        app.active = Some(0);
        app.selected = 0;
        app.right_panel_active_tab = RightPanelTab::Issues;
        app
    }

    /// Render the full right panel into a test buffer and return it.
    fn draw_panel(app: &AppState) -> ratatui::buffer::Buffer {
        let (width, height) = (30, 8);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render_right_panel(app, frame, Rect::new(0, 0, width, height)))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, row)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn tab_header_renders_the_same_labels_the_hit_test_uses() {
        let app = issues_app();
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 0), "│ Changes │ Checks │ Issues │");
    }

    #[test]
    fn issues_tab_renders_rows_with_scroll() {
        let mut app = issues_app();
        app.repo_issues.insert(
            TEST_REPO.into(),
            crate::workspace::RepoIssues {
                issues: vec![
                    crate::workspace::RepoIssue {
                        number: 7,
                        title: "bug: first".into(),
                        url: "https://github.com/owner/proj/issues/7".into(),
                    },
                    crate::workspace::RepoIssue {
                        number: 12,
                        title: "feat: second".into(),
                        url: "https://github.com/owner/proj/issues/12".into(),
                    },
                ],
                error: None,
            },
        );

        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ #7 bug: first");
        assert_eq!(row_text(&buffer, 2), "│ #12 feat: second");

        app.right_panel_scroll = 1;
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ #12 feat: second");
    }

    #[test]
    fn issues_tab_renders_error_and_empty_states() {
        let mut app = issues_app();

        // No cache, no request in flight.
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ no issues data");

        // No cache but a fetch was requested / is in flight.
        app.right_panel_issues_requested = true;
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ loading issues…");
        app.right_panel_issues_requested = false;
        app.issues_fetch_in_flight.insert(TEST_REPO.into());
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ loading issues…");
        app.issues_fetch_in_flight.clear();

        // Cached error.
        app.repo_issues.insert(
            TEST_REPO.into(),
            crate::workspace::RepoIssues {
                issues: Vec::new(),
                error: Some("gh: not logged in".into()),
            },
        );
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ gh: not logged in");

        // Cached empty list.
        app.repo_issues.insert(
            TEST_REPO.into(),
            crate::workspace::RepoIssues {
                issues: Vec::new(),
                error: None,
            },
        );
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ no open issues");

        // Workspace without a git space.
        app.workspaces[0].cached_git_space = None;
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ not a git repository");
    }

    fn prs_app() -> AppState {
        let mut app = issues_app();
        app.right_panel_active_tab = RightPanelTab::PullRequests;
        app
    }

    fn open_pr(
        number: u64,
        title: &str,
        is_draft: bool,
        mergeable: Option<&str>,
    ) -> crate::workspace::OpenPr {
        crate::workspace::OpenPr {
            number,
            title: title.into(),
            url: format!("https://github.com/owner/proj/pull/{number}"),
            head_ref_name: format!("branch-{number}"),
            is_draft,
            mergeable: mergeable.map(String::from),
        }
    }

    #[test]
    fn prs_tab_renders_rows_with_indicators_and_scroll() {
        let mut app = prs_app();
        app.repo_open_prs.insert(
            TEST_REPO.into(),
            crate::workspace::RepoOpenPrs {
                prs: vec![
                    open_pr(42, "feat: widget", false, Some("MERGEABLE")),
                    open_pr(7, "wip: exp", true, Some("CONFLICTING")),
                    open_pr(9, "chore: x", false, None),
                ],
                error: None,
            },
        );

        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ #42 feat: widget ✓");
        assert_eq!(row_text(&buffer, 2), "│ ◌ #7 wip: exp ✗");
        assert_eq!(row_text(&buffer, 3), "│ #9 chore: x");

        app.right_panel_scroll = 1;
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ ◌ #7 wip: exp ✗");
    }

    #[test]
    fn prs_tab_renders_error_and_empty_states() {
        let mut app = prs_app();

        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ no pull request data");

        app.right_panel_prs_requested = true;
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ loading pull requests…");
        app.right_panel_prs_requested = false;
        app.prs_fetch_in_flight.insert(TEST_REPO.into());
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ loading pull requests…");
        app.prs_fetch_in_flight.clear();

        app.repo_open_prs.insert(
            TEST_REPO.into(),
            crate::workspace::RepoOpenPrs {
                prs: Vec::new(),
                error: Some("gh: not logged in".into()),
            },
        );
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ gh: not logged in");

        app.repo_open_prs.insert(
            TEST_REPO.into(),
            crate::workspace::RepoOpenPrs {
                prs: Vec::new(),
                error: None,
            },
        );
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ no open pull requests");

        app.workspaces[0].cached_git_space = None;
        let buffer = draw_panel(&app);
        assert_eq!(row_text(&buffer, 1), "│ not a git repository");
    }
}
