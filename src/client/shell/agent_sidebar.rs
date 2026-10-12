use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};

use super::*;

pub(super) struct AgentRow {
    pub(super) pane_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    /// Draw the whole row dim (ceo-bora#353): see `agent_is_idle_stale`.
    pub(super) idle_stale: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
}

/// The one agent-panel order for a single snapshot. The panel
/// (`agent_rows`), the collapsed rail and agent keyboard navigation
/// (`FocusAgent`/`PreviousAgent`/`NextAgent` in `actions.rs`) all read it,
/// and the multi-endpoint `aggregate_agent_rows` builds on it, so what the
/// keys walk is what the panel draws. An active plugin `agent.view` wins
/// as-is: its stale rows are only dimmed, never moved. Otherwise a stale
/// row (ceo-bora#353) sinks to the end of its own workspace's run in
/// `spaces` order and below every non-stale row in `priority` and
/// `recent` order (`recent_sort_key`).
/// Computed fresh on every call from the client-aged `idle_seconds`, so a
/// row that crosses the stale edge on a quiet screen re-sorts on the
/// repaint `tick_idle_ageing` requests for that crossing.
pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
) -> Vec<String> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .cloned()
            .collect();
    }
    let mut agents = snapshot
        .agents
        .iter()
        .map(|agent| (agent, agent_is_idle_stale(snapshot, agent, config)))
        .collect::<Vec<_>>();
    match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Priority => {
            agents.sort_by_key(|(agent, _)| {
                (
                    std::cmp::Reverse(status_priority(agent.agent_status)),
                    std::cmp::Reverse(agent.state_change_seq),
                )
            });
            sink_idle_stale_rows(&mut agents, |_| (), |(_, idle_stale)| *idle_stale);
        }
        crate::config::AgentPanelSortConfig::Recent => {
            agents.sort_by_cached_key(|(agent, _)| {
                recent_sort_key(
                    agent.agent_status,
                    pane_idle_seconds(snapshot, &agent.pane_id),
                    agent.state_change_seq,
                )
            });
            sink_idle_stale_rows(&mut agents, |_| (), |(_, idle_stale)| *idle_stale);
        }
        crate::config::AgentPanelSortConfig::Spaces => sink_idle_stale_rows(
            &mut agents,
            |(agent, _)| agent.workspace_id.as_str(),
            |(_, idle_stale)| *idle_stale,
        ),
    }
    agents
        .into_iter()
        .map(|(agent, _)| agent.pane_id.clone())
        .collect()
}

/// The `recent` order key (ceo-bora#354): one flat list by last activity.
/// Working and Blocked agents come first, newest `activity` first; then
/// Idle and Done agents by smallest `idle_seconds` (the client-aged value,
/// see `tick_idle_ageing`), newest `activity` breaking ties. An Idle/Done
/// agent whose idle age is unknown (`None`: e.g. a restored session, whose
/// `idle_since` is not persisted) is not claimed recent: it follows every
/// agent with a known age. `Unknown`-status agents (no live agent state)
/// come last. Inside those last two groups newest `activity` comes first,
/// and the sort is stable, so equal keys keep the snapshot order.
///
/// `activity` is the snapshot's own `state_change_seq` for a single
/// endpoint and the client-observed `agent_recency` across endpoints,
/// because each server's `state_change_seq` is its own counter and is not
/// comparable across machines. `idle_seconds` is a clock, so it is.
///
/// The ceo-bora#353 stale sink still runs after this key, over the whole
/// list as in `priority`. Inside the Idle/Done group it is a no-op (stale
/// is an edge on the same ascending `idle_seconds`, so stale rows already
/// sit at that group's tail); its only effect is to keep the #353 promise
/// that a stale row sits below every non-stale row, including the
/// unknown-age and `Unknown` ones.
pub(super) fn recent_sort_key(
    status: crate::api::schema::AgentStatus,
    idle_seconds: Option<u64>,
    activity: u64,
) -> (u8, u64, std::cmp::Reverse<u64>) {
    use crate::api::schema::AgentStatus;
    let newest = std::cmp::Reverse(activity);
    match (status, idle_seconds) {
        (AgentStatus::Working | AgentStatus::Blocked, _) => (0, 0, newest),
        (AgentStatus::Idle | AgentStatus::Done, Some(seconds)) => (1, seconds, newest),
        (AgentStatus::Idle | AgentStatus::Done, None) => (2, 0, newest),
        (AgentStatus::Unknown, _) => (3, 0, newest),
    }
}

/// The pane's client-aged `idle_seconds`, if the snapshot carries the pane.
pub(super) fn pane_idle_seconds(snapshot: &ClientShellSnapshot, pane_id: &str) -> Option<u64> {
    snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == pane_id)
        .and_then(|pane| pane.idle_seconds)
}

/// Move every `idle_stale` row to the end of its run, where a run is a
/// maximal stretch of adjacent rows with the same `run_key` (one workspace
/// in `spaces` order; the whole list, or one online/offline machine block,
/// in `priority` and `recent` order). Stable, so ties keep their relative order. The
/// only place the ceo-bora#353 sinking rule lives.
pub(super) fn sink_idle_stale_rows<T, K: PartialEq>(
    rows: &mut [T],
    run_key: impl Fn(&T) -> K,
    idle_stale: impl Fn(&T) -> bool,
) {
    let mut start = 0;
    while start < rows.len() {
        let key = run_key(&rows[start]);
        let end = rows[start..]
            .iter()
            .position(|row| run_key(row) != key)
            .map_or(rows.len(), |offset| start + offset);
        rows[start..end].sort_by_key(|row| idle_stale(row));
        start = end;
    }
}

/// Whether an agent's row is stale for the panel (ceo-bora#353): its pane
/// sits in the #352 `IdleTier::Stale` tier of the client-aged
/// `idle_seconds`, and `ui.dim_stale_agents` is on.
pub(super) fn agent_is_idle_stale(
    snapshot: &ClientShellSnapshot,
    agent: &crate::protocol::ClientShellAgent,
    config: &ClientShellConfig,
) -> bool {
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    idle_tier_dims_row(agent_idle_tier(agent, pane, config), config)
}

fn agent_idle_tier(
    agent: &crate::protocol::ClientShellAgent,
    pane: Option<&crate::protocol::ClientShellPane>,
    config: &ClientShellConfig,
) -> Option<attention::IdleTier> {
    attention::idle_tier(
        agent.agent_status,
        pane.and_then(|pane| pane.idle_seconds),
        config.idle_fresh_seconds,
        config.idle_stale_seconds,
    )
}

fn idle_tier_dims_row(idle_tier: Option<attention::IdleTier>, config: &ClientShellConfig) -> bool {
    config.dim_stale_agents && idle_tier == Some(attention::IdleTier::Stale)
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        config,
        hits,
    ) {
        return;
    }

    let rows = agent_rows(snapshot, config, None);
    render_agent_list(
        buffer,
        area,
        &rows,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |row| row.rows.len(),
        |buffer, rect, row, hits| {
            hits.agents.push((rect, row.pane_id.clone()));
            render_agent_row(buffer, rect, row, config);
        },
    );
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
        crate::config::AgentPanelSortConfig::Recent => "recent",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    true
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let gaps = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y
            .saturating_add(height)
            .saturating_add(if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            });
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

pub(super) fn agent_rows(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Vec<AgentRow> {
    ordered_agent_pane_ids(snapshot, config)
        .into_iter()
        .filter_map(|pane_id| agent_row(snapshot, &pane_id, config, machine))
        .collect()
}

pub(super) fn agent_row(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Option<AgentRow> {
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = labels
        .get(status_text(agent.agent_status))
        .map(String::as_str)
        .unwrap_or_else(|| sidebar_status_text(agent.agent_status));
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: agent
                .title
                .as_deref()
                .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
        },
        state_text,
    );
    let idle_tier = agent_idle_tier(agent, pane, config);
    let rows = idle_tiered_agent_token_rows(rows, idle_tier, agent.focused, &config.palette);
    Some(AgentRow {
        pane_id: agent.pane_id.clone(),
        status: agent.agent_status,
        focused: agent.focused,
        idle_stale: idle_tier_dims_row(idle_tier, config),
        rows,
    })
}

/// Recolor the `Agent`-kind token in each row by idle tier (ceo-bora#352),
/// via the existing per-token `SidebarTokenStyle` patch channel
/// (`apply_token_style` in `src/ui/sidebar.rs`) rather than a new style
/// parameter threaded through the shared row renderer -- `ResolvedTokenKind::Agent`
/// shares its `secondary_style` slot with `Machine`/`Tab`/`Pane`/`Branch`, so
/// a per-row style override would recolor those too. `None` (Working/
/// Blocked/Unknown) leaves the token untouched: today's hardcoded
/// `overlay0` stands, unchanged. `Plain` explicitly repaints it to the
/// focus-aware base text color -- without this, it would otherwise keep
/// that same hardcoded `overlay0`, which is not "the normal text colour"
/// the Plain tier is supposed to show. Under the 16-color `terminal` theme
/// the palette resolves to named colors (Fresh `Cyan`, Plain `Gray`/focused
/// `Reset`, Stale `Gray`), carried as-is by `SidebarTokenColor::from_palette`.
fn idle_tiered_agent_token_rows(
    rows: Vec<Vec<crate::ui::ResolvedToken>>,
    idle_tier: Option<attention::IdleTier>,
    focused: bool,
    palette: &Palette,
) -> Vec<Vec<crate::ui::ResolvedToken>> {
    let Some(tier) = idle_tier else {
        return rows;
    };
    let color = match tier {
        attention::IdleTier::Fresh => palette.teal,
        attention::IdleTier::Stale => palette.overlay0,
        attention::IdleTier::Plain => {
            if focused {
                palette.text
            } else {
                palette.subtext0
            }
        }
    };
    let fg = crate::config::SidebarTokenColor::from_palette(color);
    rows.into_iter()
        .map(|row| {
            row.into_iter()
                .map(|mut token| {
                    if matches!(token.kind, crate::ui::ResolvedTokenKind::Agent(_)) {
                        token.style.fg = Some(fg);
                    }
                    token
                })
                .collect()
        })
        .collect()
}

pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, config.status_indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        let indent = if index == 0 { 1 } else { 3 };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent))];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
    if row.idle_stale {
        // Row-level, after the tokens are drawn: token rules style one
        // token at most and never the state icon, so only this reaches the
        // icon, every token and the padding between them (ceo-bora#353).
        buffer.set_style(rect, Style::default().add_modifier(Modifier::DIM));
    }
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}
