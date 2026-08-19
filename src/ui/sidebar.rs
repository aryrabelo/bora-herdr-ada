mod tokens;

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use self::tokens::{ResolvedToken, ResolvedTokenKind, SpaceTokenContext};
use super::scrollbar::{render_scrollbar, should_show_scrollbar};
use super::status::{
    agent_icon, format_idle_age, idle_age_color, state_dot, state_label, state_label_color,
};
use super::text::{display_width, display_width_u16, truncate_end};
use crate::app::state::{AgentPanelSort, Palette};
use crate::app::{AppState, Mode};
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

const WORKSPACE_SECTION_HEADER_ROWS: u16 = 2;
const AGENT_PANEL_HEADER_ROWS: u16 = 3;

/// Glyph + style for a PR's rolled-up check status, shown after the PR badge.
fn checks_badge(
    checks: &[crate::workspace::CheckRun],
    p: &Palette,
) -> Option<(&'static str, Style)> {
    use crate::workspace::ChecksRollup;
    match crate::workspace::checks_rollup(checks)? {
        ChecksRollup::Passing => Some((" ✓", Style::default().fg(p.green))),
        ChecksRollup::Failing => Some((" ✗", Style::default().fg(p.red))),
        ChecksRollup::Pending => Some((" ●", Style::default().fg(p.yellow))),
    }
}

pub(crate) struct AgentPanelEntry {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub primary_label: String,
    pub primary_tab_label: Option<String>,
    pub agent_label: Option<String>,
    pub pane_label: Option<String>,
    pub terminal_title: Option<String>,
    pub terminal_title_stripped: Option<String>,
    pub agent_kind_label: Option<String>,
    pub agent: Option<crate::detect::Agent>,
    pub state: AgentState,
    pub seen: bool,
    pub idle_since: Option<std::time::Instant>,
    pub last_agent_state_change_seq: Option<u64>,
    pub custom_status: Option<String>,
    pub state_labels: std::collections::HashMap<String, String>,
    pub tokens: std::collections::HashMap<String, String>,
}

fn sidebar_section_heights(total_h: u16, split_ratio: f32) -> (u16, u16) {
    if total_h == 0 {
        return (0, 0);
    }

    if total_h < 6 {
        let ws_h = total_h.div_ceil(2);
        return (ws_h, total_h.saturating_sub(ws_h));
    }

    let ratio = split_ratio.clamp(0.1, 0.9);
    let ws_h = (f32::from(total_h) * ratio).round() as u16;
    let ws_h = ws_h.clamp(3, total_h.saturating_sub(3));
    let detail_h = total_h.saturating_sub(ws_h);
    (ws_h, detail_h)
}

pub(crate) fn expanded_sidebar_sections(area: Rect, split_ratio: f32) -> (Rect, Rect) {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height == 0 {
        return (Rect::default(), Rect::default());
    }

    let (ws_h, detail_h) = sidebar_section_heights(content.height, split_ratio);
    let ws_area = Rect::new(content.x, content.y, content.width, ws_h);
    let detail_area = Rect::new(content.x, content.y + ws_h, content.width, detail_h);
    (ws_area, detail_area)
}

pub(crate) fn sidebar_section_divider_rect(area: Rect, split_ratio: f32) -> Rect {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height < 6 {
        return Rect::default();
    }

    let (ws_h, _) = sidebar_section_heights(content.height, split_ratio);
    Rect::new(content.x, content.y + ws_h, content.width, 1)
}

fn agent_panel_sort_label(sort: AgentPanelSort) -> &'static str {
    match sort {
        AgentPanelSort::Spaces => "grouped",
        AgentPanelSort::Priority => "priority",
    }
}

pub(crate) fn agent_panel_toggle_rect(area: Rect, sort: AgentPanelSort) -> Rect {
    agent_panel_header_label_rect(area, agent_panel_sort_label(sort))
}

fn agent_panel_header_label_rect(area: Rect, label: &str) -> Rect {
    if area.width == 0 || area.height < 2 {
        return Rect::default();
    }

    let width = display_width_u16(label).min(area.width);
    Rect::new(
        area.x + area.width.saturating_sub(width),
        area.y + 1,
        width,
        1,
    )
}

fn active_agent_view_label(app: &AppState) -> Option<&str> {
    app.agent_view_override
        .as_ref()
        .map(|view| view.label.as_deref().unwrap_or("filtered"))
}

pub(crate) fn agent_panel_entries(app: &AppState) -> Vec<AgentPanelEntry> {
    agent_panel_entries_with_runtimes(app, None)
}

pub(crate) fn all_agent_panel_entries(app: &AppState) -> Vec<AgentPanelEntry> {
    collect_agent_panel_entries_with_runtimes(app, None)
}

pub(crate) fn agent_panel_entries_from(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> Vec<AgentPanelEntry> {
    agent_panel_entries_with_runtimes(app, Some(terminal_runtimes))
}

fn agent_panel_entries_with_runtimes(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
) -> Vec<AgentPanelEntry> {
    let mut entries = collect_agent_panel_entries_with_runtimes(app, terminal_runtimes);
    // `apply_agent_view`'s fallback (no explicit sort spec) re-sorts newest-first
    // within a tier, conflicting with the fork's oldest-first tie-break applied
    // in `collect_agent_panel_entries_with_runtimes`. Only invoke it when an
    // override is actually active (filtering and/or an explicit custom sort).
    if app.agent_view_override.is_some() {
        crate::app::agent_view::apply_agent_view(app, &mut entries);
    }
    entries
}

fn collect_agent_panel_entries_with_runtimes(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
) -> Vec<AgentPanelEntry> {
    let empty_runtimes;
    let terminal_runtimes = match terminal_runtimes {
        Some(terminal_runtimes) => terminal_runtimes,
        None => {
            empty_runtimes = TerminalRuntimeRegistry::new();
            &empty_runtimes
        }
    };

    let mut entries: Vec<AgentPanelEntry> = app
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(ws_idx, ws)| {
            let multi_tab = ws.tabs.len() > 1;
            let workspace_label = ws.display_name_from(&app.terminals, terminal_runtimes);
            ws.pane_details(&app.terminals)
                .into_iter()
                .map(move |detail| {
                    let show_tab = multi_tab
                        || ws
                            .tabs
                            .get(detail.tab_idx)
                            .is_some_and(|tab| !tab.is_auto_named());
                    AgentPanelEntry {
                        ws_idx,
                        tab_idx: detail.tab_idx,
                        pane_id: detail.pane_id,
                        primary_label: workspace_label.clone(),
                        primary_tab_label: show_tab.then_some(detail.tab_label),
                        pane_label: detail.pane_label,
                        terminal_title: detail.terminal_title,
                        terminal_title_stripped: detail.terminal_title_stripped,
                        agent_label: Some(detail.agent_label),
                        agent_kind_label: detail.agent_kind_label,
                        agent: detail.agent,
                        state: detail.state,
                        seen: detail.seen,
                        idle_since: detail.idle_since,
                        last_agent_state_change_seq: detail.last_agent_state_change_seq,
                        custom_status: detail.custom_status,
                        state_labels: detail.state_labels,
                        tokens: detail.tokens,
                    }
                })
        })
        .collect();

    if matches!(app.agent_panel_sort, AgentPanelSort::Priority) {
        entries.sort_by_key(|entry| {
            (
                std::cmp::Reverse(workspace_attention_priority(entry.state, entry.seen)),
                // Oldest state change first: the agent waiting the longest
                // tops its tier; panes without a recorded change sort last.
                entry.last_agent_state_change_seq.unwrap_or(u64::MAX),
            )
        });
    }

    entries
}

pub(super) fn agent_panel_status_key(state: AgentState, seen: bool) -> &'static str {
    match (state, seen) {
        (AgentState::Idle, false) => "done",
        (AgentState::Idle, true) => "idle",
        (AgentState::Working, _) => "working",
        (AgentState::Blocked, _) => "blocked",
        (AgentState::Unknown, _) => "unknown",
    }
}

fn resolved_agent_rows(app: &AppState, entry: &AgentPanelEntry) -> Vec<Vec<ResolvedToken>> {
    let label = entry
        .state_labels
        .get(agent_panel_status_key(entry.state, entry.seen))
        .map(String::as_str)
        .unwrap_or_else(|| state_label(entry.state, entry.seen));
    tokens::agent_rows(&app.sidebar_agents, entry, label)
}

fn resolved_token_spans(
    resolved: &[ResolvedToken],
    state_icon: (&str, Style),
    state_text_style: Style,
    workspace_style: Style,
    secondary_style: Style,
    custom_style: Style,
    p: &Palette,
    max_width: usize,
) -> Vec<Span<'static>> {
    let fixed_widths = resolved
        .iter()
        .map(|token| match &token.kind {
            ResolvedTokenKind::StateIcon => display_width(state_icon.0),
            ResolvedTokenKind::GitStatus { ahead, behind } => {
                usize::from(*ahead > 0) * display_width(&format!("↑{ahead}"))
                    + usize::from(*behind > 0) * display_width(&format!("↓{behind}"))
                    + usize::from(*ahead > 0 && *behind > 0)
            }
            _ => 0,
        })
        .collect::<Vec<_>>();
    let flexible_widths = resolved
        .iter()
        .map(|token| match &token.kind {
            ResolvedTokenKind::StateText(text)
            | ResolvedTokenKind::Workspace(text)
            | ResolvedTokenKind::Tab(text)
            | ResolvedTokenKind::Pane(text)
            | ResolvedTokenKind::Agent(text)
            | ResolvedTokenKind::TerminalTitle(text)
            | ResolvedTokenKind::Branch(text)
            | ResolvedTokenKind::Custom(text) => display_width(text),
            _ => 0,
        })
        .collect::<Vec<_>>();
    let minimum_width = |active: &[bool]| {
        let indices = active
            .iter()
            .enumerate()
            .filter_map(|(index, active)| active.then_some(index))
            .collect::<Vec<_>>();
        let content = indices
            .iter()
            .map(|index| fixed_widths[*index] + usize::from(flexible_widths[*index] > 0))
            .sum::<usize>();
        let separators = indices
            .windows(2)
            .map(|pair| display_width(tokens::separator(&resolved[pair[0]], &resolved[pair[1]])))
            .sum::<usize>();
        content + separators
    };
    let mut active = resolved.iter().map(|_| true).collect::<Vec<_>>();
    if minimum_width(&active) > max_width {
        for (index, width) in flexible_widths.iter().enumerate() {
            if *width > 0 {
                active[index] = false;
            }
        }
        for index in (0..resolved.len()).rev() {
            if flexible_widths[index] == 0 {
                continue;
            }
            active[index] = true;
            if minimum_width(&active) > max_width {
                active[index] = false;
            }
        }
    }
    let visible_indices = active
        .iter()
        .enumerate()
        .filter_map(|(index, active)| active.then_some(index))
        .collect::<Vec<_>>();
    let separator_width = visible_indices
        .windows(2)
        .map(|pair| display_width(tokens::separator(&resolved[pair[0]], &resolved[pair[1]])))
        .sum::<usize>();
    let fixed_width = visible_indices
        .iter()
        .map(|index| fixed_widths[*index])
        .sum::<usize>();
    let mut budgets = flexible_widths
        .iter()
        .enumerate()
        .map(|(index, width)| usize::from(active[index] && *width > 0))
        .collect::<Vec<_>>();
    let minimum = budgets.iter().sum::<usize>();
    let mut remaining = max_width
        .saturating_sub(separator_width + fixed_width)
        .saturating_sub(minimum);
    while remaining > 0 {
        let mut grew = false;
        for (budget, width) in budgets.iter_mut().zip(&flexible_widths) {
            if *budget > 0 && *budget < *width {
                *budget += 1;
                remaining -= 1;
                grew = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !grew {
            break;
        }
    }
    let mut spans = Vec::new();
    for (position, index) in visible_indices.iter().copied().enumerate() {
        let token = &resolved[index];
        if position > 0 {
            let previous = &resolved[visible_indices[position - 1]];
            spans.push(Span::styled(
                tokens::separator(previous, token),
                Style::default().fg(p.overlay0).add_modifier(Modifier::DIM),
            ));
        }
        match &token.kind {
            ResolvedTokenKind::StateIcon => {
                spans.push(Span::styled(
                    state_icon.0.to_string(),
                    apply_token_style(state_icon.1, token.style),
                ));
            }
            ResolvedTokenKind::StateText(text) => {
                spans.push(Span::styled(
                    truncate_end(text, budgets[index]),
                    apply_token_style(state_text_style, token.style),
                ));
            }
            ResolvedTokenKind::Workspace(text) => {
                spans.push(Span::styled(
                    truncate_end(text, budgets[index]),
                    apply_token_style(workspace_style, token.style),
                ));
            }
            ResolvedTokenKind::Tab(text)
            | ResolvedTokenKind::Pane(text)
            | ResolvedTokenKind::Agent(text)
            | ResolvedTokenKind::Branch(text) => {
                spans.push(Span::styled(
                    truncate_end(text, budgets[index]),
                    apply_token_style(secondary_style, token.style),
                ));
            }
            ResolvedTokenKind::GitStatus { ahead, behind } => {
                if *ahead > 0 {
                    spans.push(Span::styled(
                        format!("↑{ahead}"),
                        apply_token_style(Style::default().fg(p.green), token.style),
                    ));
                }
                if *ahead > 0 && *behind > 0 {
                    spans.push(Span::styled(
                        " ",
                        apply_token_style(Style::default(), token.style),
                    ));
                }
                if *behind > 0 {
                    spans.push(Span::styled(
                        format!("↓{behind}"),
                        apply_token_style(Style::default().fg(p.red), token.style),
                    ));
                }
            }
            ResolvedTokenKind::TerminalTitle(text) | ResolvedTokenKind::Custom(text) => {
                spans.push(Span::styled(
                    truncate_end(text, budgets[index]),
                    apply_token_style(custom_style, token.style),
                ));
            }
        }
    }
    spans
}

fn apply_token_style(mut style: Style, patch: crate::config::SidebarTokenStyle) -> Style {
    if let Some(fg) = patch.fg {
        style = style.fg(fg.ratatui());
    }
    if let Some(bold) = patch.bold {
        style = if bold {
            style.add_modifier(Modifier::BOLD)
        } else {
            style.remove_modifier(Modifier::BOLD)
        };
    }
    if let Some(dim) = patch.dim {
        style = if dim {
            style.add_modifier(Modifier::DIM)
        } else {
            style.remove_modifier(Modifier::DIM)
        };
    }
    style
}

/// Tree rail for a workspace listed under a branch header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchRail {
    /// Loose workspace with no detected branch — no tree spine.
    None,
    /// Under an open bracket; the project spine continues down (│).
    Spine,
    /// Last row of a bracketed group — draws the closing elbow (╰──). Every
    /// group closes on a workspace row, so no rail is ever left blank.
    Close,
}

/// Per-tab aggregate dot states in tab order: (AgentState, seen).
fn tab_dot_states(
    ws: &crate::workspace::Workspace,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
) -> Vec<(AgentState, bool)> {
    let details = ws.pane_details(terminals);
    (0..ws.tabs.len())
        .map(|t| {
            details
                .iter()
                .filter(|d| d.tab_idx == t)
                .map(|d| (d.state, d.seen))
                .max_by_key(|(s, seen)| workspace_display_priority(*s, *seen))
                .unwrap_or((AgentState::Unknown, true))
        })
        .collect()
}

/// Per-tab oldest unseen-idle age in tab order, parallel to `tab_dot_states`.
fn tab_dot_idle_ages(
    ws: &crate::workspace::Workspace,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
    now: std::time::Instant,
) -> Vec<Option<std::time::Duration>> {
    let details = ws.pane_details(terminals);
    (0..ws.tabs.len())
        .map(|t| {
            details
                .iter()
                .filter(|d| {
                    d.tab_idx == t
                        && !d.seen
                        && matches!(d.state, AgentState::Idle | AgentState::Unknown)
                })
                .filter_map(|d| d.idle_since)
                .map(|since| now.saturating_duration_since(since))
                .max()
        })
        .collect()
}

/// First pane's resolved agent label for a workspace's tree row, e.g. the
/// ` @nome` badge. `Workspace::pane_details` already filters to panes with
/// SOME agent identity and prefers a registered `agent rename` name over a
/// detected agent's label (`effective_display_agent`), so the first result
/// is already correctly prioritized — nothing to redo here. Pure in-memory
/// terminal-state lookup, safe to call every render.
fn workspace_agent_label(
    ws: &crate::workspace::Workspace,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
) -> Option<String> {
    ws.pane_details(terminals)
        .into_iter()
        .next()
        .map(|detail| detail.agent_label)
}

fn workspace_attention_priority(state: AgentState, seen: bool) -> u8 {
    match (state, seen) {
        (AgentState::Blocked, _) => 4,
        (AgentState::Idle, false) => 3,
        (AgentState::Working, _) => 2,
        (AgentState::Idle, true) => 1,
        (AgentState::Unknown, _) => 0,
    }
}

/// Display-only priority for a space's aggregate dot: prefers `Working` over a
/// just-finished `Done` (Idle-unseen). Mirrors `workspace_attention_priority`
/// but does not affect sort order.
fn workspace_display_priority(state: AgentState, seen: bool) -> u8 {
    match (state, seen) {
        (AgentState::Blocked, _) => 4,
        (AgentState::Working, _) => 3,
        (AgentState::Idle, false) => 2,
        (AgentState::Idle, true) => 1,
        (AgentState::Unknown, _) => 0,
    }
}

fn space_aggregate_display_state(app: &AppState, key: &str) -> (AgentState, bool) {
    app.workspaces
        .iter()
        .filter(|ws| {
            ws.git_space()
                .is_some_and(|space| space.repo_identity == key)
        })
        .map(|ws| ws.aggregate_display_state(&app.terminals))
        .max_by_key(|(state, seen)| workspace_display_priority(*state, *seen))
        .unwrap_or((AgentState::Unknown, true))
}

/// Oldest unseen-idle age across a space's workspaces, parallel to
/// `space_aggregate_display_state`. Drives the age color of a collapsed group.
fn space_aggregate_idle_age(
    app: &AppState,
    key: &str,
    now: std::time::Instant,
) -> Option<std::time::Duration> {
    app.workspaces
        .iter()
        .filter(|ws| {
            ws.git_space()
                .is_some_and(|space| space.repo_identity == key)
        })
        .filter_map(|ws| ws.oldest_unseen_idle_age(&app.terminals, now))
        .max()
}

pub(crate) fn workspace_parent_group_state(
    app: &AppState,
    ws_idx: usize,
) -> Option<(String, bool)> {
    let space = app.workspaces.get(ws_idx)?.git_space()?;
    if space.is_linked_worktree {
        return None;
    }
    let member_count = app
        .workspaces
        .iter()
        .filter(|ws| {
            ws.git_space()
                .is_some_and(|member| member.repo_identity == space.repo_identity)
        })
        .count();
    (member_count >= 2).then(|| {
        (
            space.repo_identity.clone(),
            app.collapsed_space_keys.contains(&space.repo_identity),
        )
    })
}

/// Strip `worktree/` prefix from a branch label for display.
fn branch_display_label(branch: &str) -> &str {
    branch.strip_prefix("worktree/").unwrap_or(branch)
}

/// Folded first-branch summary carried inline on a top-level project header
/// (`╭─name [label] ↑a ↓b`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectHeaderBranch {
    pub label: String,
    pub ahead: usize,
    pub behind: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceListEntry {
    Workspace {
        ws_idx: usize,
        indented: bool,
        rail: BranchRail,
    },
    /// A collapsible group header row: a user-defined visual group, or a
    /// synthesized repo header when no main checkout of the repo is open.
    GroupHeader { name: String, collapse_key: String },
    /// Repo/project header (no chevron). Top-level headers open the bracket
    /// rail (`╭─`) and fold the group's first branch into `branch`; headers
    /// nested under a visual group are `indented` and carry `branch: None`.
    ProjectHeader {
        name: String,
        collapse_key: String,
        indented: bool,
        branch: Option<ProjectHeaderBranch>,
    },
    /// Branch sub-header inside a project group. Headers draw `├── ` (or
    /// `╰── ` when `last`); all connectors are 4 cells wide.
    ///
    /// `ws_idx`: a branch holding exactly one auto-named worktree printed
    /// its name on this header AND again on the child `Workspace` row below
    /// (both derive from the same checkout). `Some(idx)` folds that single
    /// workspace INTO the header — the row renders and clicks like a
    /// `Workspace` row (dot, idle age, selection highlight) instead of a
    /// plain label, and no separate `Workspace` entry is emitted for it.
    /// `None` is the plain non-clickable label: a branch with 2+ workspaces
    /// (a worktree can host two), or a workspace the user renamed by hand.
    BranchHeader {
        label: String,
        ahead: usize,
        behind: usize,
        indented: bool,
        last: bool,
        ws_idx: Option<usize>,
    },
    /// Collapsible header for the bottom "Hidden" section; `count` is the
    /// number of temporarily-hidden workspaces beneath it.
    HiddenHeader { count: usize },
}

/// Derive the repo-header "+" (create worktree) hit areas from the sidebar
/// group-header areas: only headers whose collapse key is a raw repo identity
/// (not a `vg:` visual group) get a 3-cell "+" at the row's trailing edge.
/// Shared by `compute_view` and mouse hit-testing.
pub(crate) fn worktree_new_hit_areas_from_headers(
    headers: &[crate::app::state::GroupHeaderCardArea],
) -> Vec<crate::app::state::WorktreeNewHitArea> {
    headers
        .iter()
        .filter(|header| {
            !header.collapse_key.starts_with("vg:") && header.collapse_key != "hidden:"
        })
        .filter(|header| header.rect.width >= 3)
        .map(|header| crate::app::state::WorktreeNewHitArea {
            repo_identity: header.collapse_key.clone(),
            rect: Rect::new(header.rect.x + header.rect.width - 3, header.rect.y, 3, 1),
        })
        .collect()
}

/// Shared row-height for a single entry. ALL three lockstep passes
/// (`workspace_list_visible_count`, `compute_workspace_list_areas`,
/// `render_workspace_list`) MUST call this. Never duplicate height logic.
fn entry_row_height(
    entry: &WorkspaceListEntry,
    _entries: &[WorkspaceListEntry],
    _idx: usize,
) -> u16 {
    match entry {
        WorkspaceListEntry::GroupHeader { .. } => 1,
        WorkspaceListEntry::ProjectHeader { .. } => 1,
        WorkspaceListEntry::BranchHeader { .. } => 1,
        WorkspaceListEntry::Workspace { .. } => 1,
        WorkspaceListEntry::HiddenHeader { .. } => 1,
    }
}

pub(crate) fn normalized_workspace_scroll(app: &AppState, area: Rect, requested: usize) -> usize {
    let ws_area = workspace_list_rect(area, app.sidebar_section_split);
    let body = workspace_list_body_rect(app, ws_area, false);
    if body.height == 0 {
        return requested;
    }

    let entry_count = workspace_list_entries(app).len();
    if entry_count == 0 {
        0
    } else {
        requested.min(entry_count.saturating_sub(1))
    }
}

/// Display label for an indented (grouped child) workspace row in the mobile
/// switcher: auto-named children show their short branch name.
pub(crate) fn grouped_child_display_label(
    label: &str,
    branch: Option<&str>,
    has_custom_name: bool,
) -> String {
    if has_custom_name {
        return label.to_string();
    }
    let Some(branch) = branch else {
        return label.to_string();
    };
    branch
        .strip_prefix("worktree/")
        .unwrap_or(branch)
        .to_string()
}

/// A workspace auto-filed as a channel: `#`-labelled and not placed in a group
/// by hand.
fn is_auto_channel(ws: &crate::workspace::Workspace) -> bool {
    ws.visual_group.is_none()
        && ws
            .custom_name
            .as_deref()
            .is_some_and(|name| name.starts_with('#'))
}

/// Group a workspace belongs to for sidebar purposes. An explicit group set by
/// the user always wins, so a channel can still be filed somewhere else.
///
/// `channel_group` is the configured display name; the rule that decides WHAT is
/// a channel keys off the `#` label, never off this string, so renaming the group
/// cannot change which workspaces land in it.
fn effective_visual_group<'a>(
    ws: &'a crate::workspace::Workspace,
    channel_group: &'a str,
) -> Option<&'a str> {
    if let Some(group) = ws.visual_group.as_deref() {
        return Some(group);
    }
    is_auto_channel(ws).then_some(channel_group)
}

/// Git space a workspace contributes to repo grouping, which for a channel is
/// none at all.
///
/// A channel lives in whatever checkout hosted it, and orc hosts every channel
/// in the orchestrator hub — a NON-linked checkout. Letting that count as repo
/// membership does two wrong things at once: the channel joins the repo's branch
/// bracket, and, being a non-linked checkout with a group of its own, it drags
/// the repo's entire member list inside the channel group.
fn grouping_git_space(
    ws: &crate::workspace::Workspace,
) -> Option<&crate::workspace::GitSpaceMetadata> {
    if is_auto_channel(ws) {
        return None;
    }
    ws.git_space()
}

pub(crate) fn workspace_list_entries(app: &AppState) -> Vec<WorkspaceListEntry> {
    workspace_list_entries_inner(app, false)
}

/// Like [`workspace_list_entries`] but always expands collapsed groups. The
/// mobile switcher has no collapse affordance and always shows the full tree.
pub(crate) fn workspace_list_entries_expanded(app: &AppState) -> Vec<WorkspaceListEntry> {
    workspace_list_entries_inner(app, true)
}

pub(crate) fn next_entry_is_indented_workspace(entries: &[WorkspaceListEntry], idx: usize) -> bool {
    matches!(
        entries.get(idx.saturating_add(1)),
        Some(WorkspaceListEntry::Workspace { indented: true, .. })
    )
}

fn workspace_list_entries_inner(app: &AppState, force_expanded: bool) -> Vec<WorkspaceListEntry> {
    if !app.group_workspaces_by_repo {
        // Flat sidebar: one row per workspace in workspace-vec order (which
        // flat-mode drags mutate), with no grouping at all -- repo brackets,
        // the channels group, and visual groups all dissolve while this is
        // off. Workspaces hidden individually stay hidden via the shared
        // post-filter; a repo hidden group-level in grouped mode has no
        // header here to keep it hidden, so its rows reappear until grouping
        // is turned back on.
        let entries: Vec<WorkspaceListEntry> = (0..app.workspaces.len())
            .map(|ws_idx| WorkspaceListEntry::Workspace {
                ws_idx,
                indented: false,
                rail: BranchRail::None,
            })
            .collect();
        if force_expanded {
            return entries;
        }
        return apply_hidden_filter(app, &std::collections::HashSet::new(), entries);
    }

    let mut members_by_key = std::collections::HashMap::<String, Vec<usize>>::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        if let Some(space) = grouping_git_space(ws) {
            members_by_key
                .entry(space.repo_identity.clone())
                .or_default()
                .push(ws_idx);
        }
    }
    let grouped_keys = members_by_key
        .iter()
        .filter(|(_, members)| members.len() >= 2)
        .map(|(key, _)| key.clone())
        .collect::<std::collections::HashSet<_>>();

    let visible_group_idx = if matches!(app.mode, Mode::Navigate) {
        Some(app.selected)
    } else {
        app.active
    };
    let active_group = visible_group_idx.and_then(|idx| {
        app.workspaces
            .get(idx)
            .and_then(|ws| ws.git_space())
            .map(|space| space.repo_identity.clone())
    });

    // --- Visual group setup ---
    let mut visual_group_members = std::collections::HashMap::<String, Vec<usize>>::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        if let Some(group_name) = effective_visual_group(ws, &app.channel_group_name) {
            visual_group_members
                .entry(group_name.to_owned())
                .or_default()
                .push(ws_idx);
        }
    }
    let in_visual_group: std::collections::HashSet<usize> = visual_group_members
        .values()
        .flat_map(|v| v.iter().copied())
        .collect();

    // Pre-compute: worktree children whose parent is in a visual group are consumed
    // by the visual group handler and must be skipped in the main loop.
    let mut consumed = std::collections::HashSet::<usize>::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        if effective_visual_group(ws, &app.channel_group_name).is_some() {
            if let Some(space) = grouping_git_space(ws)
                .filter(|s| grouped_keys.contains(&s.repo_identity) && !s.is_linked_worktree)
            {
                if let Some(members) = members_by_key.get(&space.repo_identity) {
                    for &m in members {
                        if m != ws_idx
                            && app.workspaces.get(m).is_some_and(|w| {
                                effective_visual_group(w, &app.channel_group_name).is_none()
                            })
                        {
                            consumed.insert(m);
                        }
                    }
                }
            }
        }
    }

    let mut emitted_worktree_groups = std::collections::HashSet::<String>::new();
    let mut emitted_visual_groups = std::collections::HashSet::<String>::new();
    let mut entries = Vec::new();

    // Channels lead, then everything else in its own order.
    //
    // A group is emitted where its FIRST member sits, so with creation order the
    // channel block landed in the middle of the repo groups — anything created
    // after the first `#` workspace ended up below the channels, which reads as
    // if that repo belonged under them. Channels and workspaces are two separate
    // kinds, so they get two separate blocks. The sort is stable, so the repo
    // groups keep their relative order exactly as before.
    let mut emission_order: Vec<usize> = (0..app.workspaces.len()).collect();
    emission_order.sort_by_key(|&idx| !is_auto_channel(&app.workspaces[idx]));

    for ws_idx in emission_order {
        let ws = &app.workspaces[ws_idx];
        if consumed.contains(&ws_idx) {
            continue;
        }

        let in_worktree_group = grouping_git_space(ws)
            .filter(|space| grouped_keys.contains(&space.repo_identity))
            .is_some();

        if in_worktree_group && !in_visual_group.contains(&ws_idx) {
            let Some(space) = grouping_git_space(ws) else {
                continue;
            };
            if emitted_worktree_groups.contains(&space.repo_identity) {
                continue;
            }
            emitted_worktree_groups.insert(space.repo_identity.clone());

            let Some(members) = members_by_key.get(&space.repo_identity) else {
                continue;
            };
            // Always synthesize a project header (the repo label); every checkout
            // of the repo becomes a member inside a branch bracket beneath it.
            let collapsed =
                !force_expanded && app.collapsed_space_keys.contains(&space.repo_identity);
            entries.push(WorkspaceListEntry::ProjectHeader {
                name: space.repo_name.clone(),
                collapse_key: space.repo_identity.clone(),
                indented: false,
                branch: None,
            });
            if collapsed {
                if let Some(active_idx) = visible_group_idx
                    .filter(|_| active_group.as_deref() == Some(space.repo_identity.as_str()))
                {
                    entries.push(WorkspaceListEntry::Workspace {
                        ws_idx: active_idx,
                        indented: true,
                        rail: BranchRail::None,
                    });
                }
            } else {
                emit_branch_subgroups(app, members, true, true, &mut entries);
            }
            continue;
        }

        if let Some(space) = grouping_git_space(ws).filter(|_| in_worktree_group) {
            if emitted_worktree_groups.contains(&space.repo_identity) {
                continue;
            }
        }

        // --- Visual group handling ---
        if in_visual_group.contains(&ws_idx) {
            let group_name = effective_visual_group(ws, &app.channel_group_name)
                .expect("in_visual_group only set for workspaces with a group");
            if emitted_visual_groups.insert(group_name.to_owned()) {
                let vg_key = format!("vg:{group_name}");
                let collapsed = !force_expanded && app.collapsed_space_keys.contains(&vg_key);
                entries.push(WorkspaceListEntry::GroupHeader {
                    name: group_name.to_owned(),
                    collapse_key: vg_key,
                });
                if !collapsed {
                    if let Some(vg_members) = visual_group_members.get(group_name) {
                        let last_member = vg_members.len().saturating_sub(1);
                        for (position, &member_idx) in vg_members.iter().enumerate() {
                            let member_ws = &app.workspaces[member_idx];
                            if is_auto_channel(member_ws) {
                                // A channel row is just the channel: no repo header
                                // and no branch bracket, because the checkout that
                                // hosts it is not what the row is about. The rail
                                // still closes on the group's last row.
                                entries.push(WorkspaceListEntry::Workspace {
                                    ws_idx: member_idx,
                                    indented: true,
                                    rail: if position == last_member {
                                        BranchRail::Close
                                    } else {
                                        BranchRail::Spine
                                    },
                                });
                                continue;
                            }
                            let repo = grouping_git_space(member_ws)
                                .filter(|s| grouped_keys.contains(&s.repo_identity))
                                .map(|s| (s.repo_identity.clone(), s.repo_name.clone()));

                            if let Some((repo_id, label)) = repo {
                                // One synthesized project header per repo group; skip
                                // members whose group was already emitted (clones/worktrees).
                                if !emitted_worktree_groups.insert(repo_id.clone()) {
                                    continue;
                                }
                                let wt_collapsed =
                                    !force_expanded && app.collapsed_space_keys.contains(&repo_id);
                                entries.push(WorkspaceListEntry::ProjectHeader {
                                    name: label,
                                    collapse_key: repo_id.clone(),
                                    indented: true,
                                    branch: None,
                                });
                                if !wt_collapsed {
                                    if let Some(members) = members_by_key.get(&repo_id) {
                                        emit_branch_subgroups(
                                            app,
                                            members,
                                            true,
                                            false,
                                            &mut entries,
                                        );
                                    }
                                }
                            } else {
                                if let Some(space) = member_ws.git_space() {
                                    entries.push(WorkspaceListEntry::ProjectHeader {
                                        name: space.repo_name.clone(),
                                        collapse_key: space.repo_identity.clone(),
                                        indented: true,
                                        branch: None,
                                    });
                                }
                                emit_branch_subgroups(
                                    app,
                                    &[member_idx],
                                    true,
                                    false,
                                    &mut entries,
                                );
                            }
                        }
                    }
                }
            }
            continue;
        }

        // --- Flat (ungrouped) workspace: project header (if git) + branch bracket ---
        let flat_has_header = if let Some(space) = ws.git_space() {
            entries.push(WorkspaceListEntry::ProjectHeader {
                name: space.repo_name.clone(),
                collapse_key: space.repo_identity.clone(),
                indented: false,
                branch: None,
            });
            true
        } else {
            false
        };
        emit_branch_subgroups(app, &[ws_idx], false, flat_has_header, &mut entries);
    }
    if force_expanded {
        return entries;
    }
    apply_hidden_filter(app, &grouped_keys, entries)
}

/// Post-process the raw entry list (desktop only): drop temporarily-hidden
/// workspaces and any group/branch header whose members all became hidden,
/// then append a collapsible "Hidden" section. Never applied to the mobile
/// (force-expanded) list.
fn apply_hidden_filter(
    app: &AppState,
    grouped_keys: &std::collections::HashSet<String>,
    raw: Vec<WorkspaceListEntry>,
) -> Vec<WorkspaceListEntry> {
    let level = |e: &WorkspaceListEntry| match e {
        WorkspaceListEntry::GroupHeader { .. } | WorkspaceListEntry::HiddenHeader { .. } => 0u8,
        WorkspaceListEntry::ProjectHeader { .. } => 1,
        WorkspaceListEntry::BranchHeader { .. } => 2,
        WorkspaceListEntry::Workspace { .. } => 3,
    };
    let ws_hidden = |ws_idx: usize| -> bool {
        let Some(ws) = app.workspaces.get(ws_idx) else {
            return false;
        };
        if app.is_hidden(&AppState::workspace_hide_key(ws)) {
            return true;
        }
        if let Some(group) = effective_visual_group(ws, &app.channel_group_name) {
            if app.is_hidden(&format!("vg:{group}")) {
                return true;
            }
        }
        if let Some(space) = ws.git_space() {
            if grouped_keys.contains(&space.repo_identity) && app.is_hidden(&space.repo_identity) {
                return true;
            }
        }
        false
    };

    // Mark, per header, whether it had any workspace child in the raw list and
    // whether any of those children survive. A collapsed header legitimately
    // has no children and must be kept.
    let n = raw.len();
    let mut had_child = vec![false; n];
    let mut has_kept_child = vec![false; n];
    let mut open: Vec<usize> = Vec::new();
    for (i, entry) in raw.iter().enumerate() {
        let lvl = level(entry);
        while let Some(&top) = open.last() {
            if level(&raw[top]) >= lvl {
                open.pop();
            } else {
                break;
            }
        }
        match entry {
            WorkspaceListEntry::Workspace { ws_idx, .. } => {
                let hidden = ws_hidden(*ws_idx);
                for &h in &open {
                    had_child[h] = true;
                    has_kept_child[h] |= !hidden;
                }
            }
            WorkspaceListEntry::GroupHeader { .. }
            | WorkspaceListEntry::ProjectHeader { .. }
            | WorkspaceListEntry::BranchHeader { .. } => open.push(i),
            WorkspaceListEntry::HiddenHeader { .. } => {}
        }
    }

    let mut result = Vec::with_capacity(n);
    let mut hidden_ws: Vec<usize> = Vec::new();
    for (i, entry) in raw.into_iter().enumerate() {
        match &entry {
            WorkspaceListEntry::Workspace { ws_idx, .. } => {
                if ws_hidden(*ws_idx) {
                    hidden_ws.push(*ws_idx);
                } else {
                    result.push(entry);
                }
            }
            WorkspaceListEntry::GroupHeader { collapse_key, .. }
            | WorkspaceListEntry::ProjectHeader { collapse_key, .. } => {
                let drop = app.is_hidden(collapse_key) || (had_child[i] && !has_kept_child[i]);
                if !drop {
                    result.push(entry);
                }
            }
            WorkspaceListEntry::BranchHeader { .. } => {
                if !had_child[i] || has_kept_child[i] {
                    result.push(entry);
                }
            }
            WorkspaceListEntry::HiddenHeader { .. } => result.push(entry),
        }
    }

    if !hidden_ws.is_empty() {
        result.push(WorkspaceListEntry::HiddenHeader {
            count: hidden_ws.len(),
        });
        if app.hidden_section_expanded {
            for ws_idx in hidden_ws {
                result.push(WorkspaceListEntry::Workspace {
                    ws_idx,
                    indented: true,
                    rail: BranchRail::None,
                });
            }
        }
    }

    result
}

/// Emit branch sub-groups for a list of project-group member indices.
///
/// `bracket` is true when THIS call is rooted by a just-pushed top-level
/// project header: the first branch folds into that header and the last branch
/// closes the rounded bracket. When false (nested visual-group repos, or flat
/// non-git workspaces) the legacy header-per-branch layout is used.
fn emit_branch_subgroups(
    app: &AppState,
    member_indices: &[usize],
    indented: bool,
    bracket: bool,
    entries: &mut Vec<WorkspaceListEntry>,
) {
    let mut branch_order: Vec<String> = Vec::new();
    let mut by_branch = std::collections::HashMap::<String, Vec<usize>>::new();
    let mut no_branch: Vec<usize> = Vec::new();
    for &idx in member_indices {
        if let Some(branch) = app.workspaces[idx].branch() {
            if !by_branch.contains_key(&branch) {
                branch_order.push(branch.clone());
            }
            by_branch.entry(branch).or_default().push(idx);
        } else {
            no_branch.push(idx);
        }
    }

    let bracketed = bracket;

    let branch_meta = |members: &[usize]| -> (usize, usize) {
        members
            .iter()
            .find_map(|&i| app.workspaces[i].git_ahead_behind())
            .unwrap_or((0, 0))
    };

    // Bracket mode: branchless members stay INSIDE the bracket, emitted right
    // after the folded first-branch members. The bracket closes (╰──) on the
    // last of these rows only when no further branch headers follow.
    let has_header_branches = branch_order.len() > usize::from(!branch_order.is_empty());

    // Fold the first branch group into the preceding project header.
    let folded = bracketed && !branch_order.is_empty();
    if folded {
        let first = &branch_order[0];
        let members = &by_branch[first];
        let (ahead, behind) = branch_meta(members);
        if let Some(WorkspaceListEntry::ProjectHeader { branch, .. }) = entries.last_mut() {
            *branch = Some(ProjectHeaderBranch {
                label: branch_display_label(first).to_string(),
                ahead,
                behind,
            });
        }
    }

    if bracketed {
        // Rows directly under the header: folded-branch members, then loose
        // (branchless) members.
        let mut under_header: Vec<usize> = Vec::new();
        if folded {
            under_header.extend_from_slice(&by_branch[&branch_order[0]]);
        }
        under_header.extend_from_slice(&no_branch);
        let last_member = under_header.len().saturating_sub(1);
        for (k, &idx) in under_header.iter().enumerate() {
            let rail = if !has_header_branches && k == last_member {
                BranchRail::Close
            } else {
                BranchRail::Spine
            };
            entries.push(WorkspaceListEntry::Workspace {
                ws_idx: idx,
                indented,
                rail,
            });
        }
    }

    // Remaining branches become header rows. The bracket closes on the group's
    // last ROW, never on a header: a header with members beneath it draws a tee
    // and the final member draws the elbow, so the rail runs unbroken to the
    // bottom of the group. Closing on the header instead left every row under
    // the last branch hanging with no rail beside it.
    let start = usize::from(folded);
    let header_branches = &branch_order[start..];
    for (bi, branch) in header_branches.iter().enumerate() {
        let members = &by_branch[branch];
        let (ahead, behind) = branch_meta(members);
        let is_last_branch = bracketed && bi + 1 == header_branches.len();
        // A branch with exactly one workspace whose name REPEATS the branch
        // prints the identical string twice: once as the header label, once as
        // the child row below it. Fold that workspace INTO the header instead.
        //
        // The test is the repetition itself, not how the name was set. An
        // auto-named checkout repeats by construction; a workspace renamed by
        // hand to the same string repeats just as visibly, and one measured live
        // does exactly that. A workspace named something else keeps its own row,
        // and 2+ workspaces always keep the header-plus-rows shape, because a
        // worktree can host two workspaces and the header then groups them.
        let branch_label = branch_display_label(branch);
        let repeats_branch = |ws: &crate::workspace::Workspace| match ws.custom_name.as_deref() {
            None => true,
            Some(name) => {
                name == branch_label
                    || branch_label
                        .rsplit('/')
                        .next()
                        .is_some_and(|short| name == short)
            }
        };
        let collapse_idx = match members.as_slice() {
            [only] if repeats_branch(&app.workspaces[*only]) => Some(*only),
            _ => None,
        };
        entries.push(WorkspaceListEntry::BranchHeader {
            label: branch_display_label(branch).to_string(),
            ahead,
            behind,
            // Top-level bracket headers draw the bracket tee/elbow at column 0;
            // nested ones keep the legacy indented connector.
            indented: indented && !bracketed,
            // A childless last branch, or one collapsed into this header,
            // closes the bracket itself — there is no child row left to.
            last: is_last_branch && (members.is_empty() || collapse_idx.is_some()),
            ws_idx: collapse_idx,
        });
        if collapse_idx.is_none() {
            let last_member = members.len().saturating_sub(1);
            for (mi, &idx) in members.iter().enumerate() {
                let rail = if is_last_branch && mi == last_member {
                    BranchRail::Close
                } else {
                    BranchRail::Spine
                };
                entries.push(WorkspaceListEntry::Workspace {
                    ws_idx: idx,
                    indented,
                    rail,
                });
            }
        }
    }

    if !bracketed {
        for &idx in &no_branch {
            entries.push(WorkspaceListEntry::Workspace {
                ws_idx: idx,
                indented,
                rail: BranchRail::None,
            });
        }
    }
}

pub(crate) fn workspace_list_rect(area: Rect, split_ratio: f32) -> Rect {
    let (ws_area, _) = expanded_sidebar_sections(area, split_ratio);
    ws_area
}

/// Rows the sidebar Programs launcher band occupies: one per pane-mode
/// `.bora.toml` command for the active workspace, plus the fixed
/// "+ run command…" row.
pub(crate) fn sidebar_program_row_count(app: &AppState) -> u16 {
    app.sidebar_program_commands().len() as u16 + 1
}

/// Hit/render area for the sidebar Programs launcher band: sits directly
/// above the workspace list's "new"/"menu" footer row, within `ws_area`
/// (the first section returned by `expanded_sidebar_sections`). Reserving
/// this band is `workspace_list_body_rect`'s job too — both MUST agree on
/// `sidebar_program_row_count`, or scrolling and hit-testing will disagree
/// about where the workspace list body ends.
pub(crate) fn sidebar_programs_band_rect(app: &AppState, ws_area: Rect) -> Rect {
    if ws_area.height == 0 {
        return Rect::default();
    }
    let footer_rows = 1u16;
    let rows = sidebar_program_row_count(app).min(ws_area.height.saturating_sub(footer_rows));
    let y = ws_area.y + ws_area.height - footer_rows - rows;
    Rect::new(ws_area.x, y, ws_area.width, rows)
}

pub(crate) fn workspace_list_body_rect(app: &AppState, area: Rect, has_scrollbar: bool) -> Rect {
    if area.width == 0 || area.height <= WORKSPACE_SECTION_HEADER_ROWS {
        return Rect::default();
    }

    let programs_rows = sidebar_programs_band_rect(app, area).height;
    let body_y = area.y.saturating_add(WORKSPACE_SECTION_HEADER_ROWS);
    let footer_y = (area.y + area.height).saturating_sub(1 + programs_rows);
    let body_height = footer_y.saturating_sub(body_y);
    let body_width = area.width.saturating_sub(u16::from(has_scrollbar));
    Rect::new(area.x, body_y, body_width, body_height)
}

fn workspace_list_visible_count(app: &AppState, area: Rect, scroll: usize) -> usize {
    let body = workspace_list_body_rect(app, area, false);
    if body.width == 0 || body.height == 0 {
        return 0;
    }

    let mut used_rows = 0u16;
    let mut visible = 0usize;
    let entries = workspace_list_entries(app);
    for (entry_idx, entry) in entries.iter().enumerate().skip(scroll) {
        let needed = entry_row_height(entry, &entries, entry_idx);
        if used_rows.saturating_add(needed) > body.height {
            break;
        }
        used_rows = used_rows.saturating_add(needed);
        visible += 1;
    }
    visible
}

pub(crate) fn workspace_list_scroll_metrics(
    app: &AppState,
    area: Rect,
) -> crate::pane::ScrollMetrics {
    let entries = workspace_list_entries(app);
    let total_rows = entries.len();
    let scroll = app.workspace_scroll.min(total_rows.saturating_sub(1));
    let viewport_rows = workspace_list_visible_count(app, area, scroll);
    let max_offset_from_bottom = total_rows.saturating_sub(viewport_rows);
    let offset_from_bottom = total_rows
        .saturating_sub(scroll)
        .saturating_sub(viewport_rows);

    crate::pane::ScrollMetrics {
        offset_from_bottom,
        max_offset_from_bottom,
        viewport_rows,
    }
}

pub(crate) fn workspace_list_scrollbar_rect(app: &AppState, area: Rect) -> Option<Rect> {
    let metrics = workspace_list_scroll_metrics(app, area);
    let body = workspace_list_body_rect(app, area, true);
    (should_show_scrollbar(metrics) && body.width > 0 && body.height > 0).then_some(Rect::new(
        area.x + area.width.saturating_sub(1),
        body.y,
        1,
        body.height,
    ))
}

pub(crate) fn agent_panel_body_rect(area: Rect, has_scrollbar: bool) -> Rect {
    if area.width == 0 || area.height <= AGENT_PANEL_HEADER_ROWS {
        return Rect::default();
    }

    let body_y = area.y.saturating_add(AGENT_PANEL_HEADER_ROWS);
    let body_height = (area.y + area.height).saturating_sub(body_y);
    let body_width = area.width.saturating_sub(u16::from(has_scrollbar));
    Rect::new(area.x, body_y, body_width, body_height)
}

/// Rows one agent entry occupies in the panel body, driven by how many
/// rows `[ui.sidebar.agents] rows` resolves to for this entry (an entry
/// with custom tokens can be taller or shorter than the two-row default).
pub(crate) fn agent_entry_height_in_body(
    app: &AppState,
    entry: &AgentPanelEntry,
    body_height: u16,
) -> u16 {
    (resolved_agent_rows(app, entry)
        .len()
        .max(1)
        .min(u16::MAX as usize) as u16)
        .min(body_height)
}

pub(crate) fn agent_entry_gap(app: &AppState, entry_idx: usize, entry_count: usize) -> u16 {
    if entry_idx + 1 < entry_count {
        app.sidebar_agents.row_gap
    } else {
        0
    }
}

fn agent_panel_visible_count_from(app: &AppState, area: Rect, scroll: usize) -> usize {
    let body = agent_panel_body_rect(area, false);
    if body.width == 0 || body.height == 0 {
        return 0;
    }

    let mut used_rows = 0u16;
    let mut visible = 0usize;
    let entries = agent_panel_entries(app);
    for (index, entry) in entries.iter().enumerate().skip(scroll) {
        let height = agent_entry_height_in_body(app, entry, body.height);
        if used_rows.saturating_add(height) > body.height {
            break;
        }
        used_rows = used_rows.saturating_add(height);
        visible += 1;
        used_rows = used_rows
            .saturating_add(agent_entry_gap(app, index, entries.len()))
            .min(body.height);
    }
    visible
}

fn agent_panel_bottom_start(app: &AppState, area: Rect) -> usize {
    let body = agent_panel_body_rect(area, false);
    let entries = agent_panel_entries(app);
    let mut used_rows = 0u16;
    let mut start = entries.len();
    for (index, entry) in entries.iter().enumerate().rev() {
        let gap = agent_entry_gap(app, index, entries.len());
        let needed = agent_entry_height_in_body(app, entry, body.height).saturating_add(gap);
        if used_rows.saturating_add(needed) > body.height {
            break;
        }
        used_rows = used_rows.saturating_add(needed);
        start = index;
    }
    start.min(entries.len().saturating_sub(1))
}

pub(crate) fn agent_panel_scroll_for_target(
    app: &AppState,
    area: Rect,
    current_scroll: usize,
    target: usize,
) -> usize {
    let max_scroll = agent_panel_bottom_start(app, area);
    if target < current_scroll {
        return target.min(max_scroll);
    }
    let mut scroll = current_scroll.min(max_scroll);
    while scroll < target {
        let visible = agent_panel_visible_count_from(app, area, scroll);
        if visible > 0 && target < scroll.saturating_add(visible) {
            break;
        }
        scroll += 1;
    }
    scroll.min(max_scroll)
}

pub(crate) fn agent_panel_scroll_metrics(app: &AppState, area: Rect) -> crate::pane::ScrollMetrics {
    let max_scroll = agent_panel_bottom_start(app, area);
    let scroll = app.agent_panel_scroll.min(max_scroll);
    let viewport_rows = agent_panel_visible_count_from(app, area, scroll);

    crate::pane::ScrollMetrics {
        offset_from_bottom: max_scroll.saturating_sub(scroll),
        max_offset_from_bottom: max_scroll,
        viewport_rows,
    }
}

pub(crate) fn agent_panel_scrollbar_rect(app: &AppState, area: Rect) -> Option<Rect> {
    let metrics = agent_panel_scroll_metrics(app, area);
    let body = agent_panel_body_rect(area, true);
    (should_show_scrollbar(metrics) && body.width > 0 && body.height > 0).then_some(Rect::new(
        area.x + area.width.saturating_sub(1),
        body.y,
        1,
        body.height,
    ))
}

pub(crate) fn compute_workspace_list_areas(
    app: &AppState,
    area: Rect,
) -> (
    Vec<crate::app::state::WorkspaceCardArea>,
    Vec<crate::app::state::GroupHeaderCardArea>,
) {
    let ws_area = workspace_list_rect(area, app.sidebar_section_split);
    if ws_area == Rect::default() {
        return (Vec::new(), Vec::new());
    }

    let metrics = workspace_list_scroll_metrics(app, ws_area);
    let body = workspace_list_body_rect(app, ws_area, should_show_scrollbar(metrics));
    if body.width == 0 || body.height == 0 {
        return (Vec::new(), Vec::new());
    }

    let scroll = app.workspace_scroll;
    let mut row_y = body.y;
    let body_bottom = body.y + body.height;
    let mut cards = Vec::new();
    let mut headers: Vec<crate::app::state::GroupHeaderCardArea> = Vec::new();

    let entries = workspace_list_entries(app);
    for (entry_idx, entry) in entries.iter().enumerate().skip(scroll) {
        let needed = entry_row_height(entry, &entries, entry_idx);
        if row_y.saturating_add(needed) > body_bottom {
            break;
        }
        match entry {
            WorkspaceListEntry::GroupHeader { name, collapse_key } => {
                headers.push(crate::app::state::GroupHeaderCardArea {
                    name: name.clone(),
                    collapse_key: collapse_key.clone(),
                    rect: Rect::new(body.x, row_y, body.width, 1),
                });
            }
            WorkspaceListEntry::ProjectHeader {
                name, collapse_key, ..
            } => {
                headers.push(crate::app::state::GroupHeaderCardArea {
                    name: name.clone(),
                    collapse_key: collapse_key.clone(),
                    rect: Rect::new(body.x, row_y, body.width, 1),
                });
            }
            WorkspaceListEntry::BranchHeader {
                ws_idx, indented, ..
            } => {
                // `Some` means this header folded its sole workspace's row
                // into itself (see `WorkspaceListEntry::BranchHeader`); it
                // must stay clickable, so push the same card a `Workspace`
                // row would have. `None` is the plain non-clickable label.
                if let Some(idx) = *ws_idx {
                    cards.push(crate::app::state::WorkspaceCardArea {
                        ws_idx: idx,
                        rect: Rect::new(body.x, row_y, body.width, 1),
                        indented: *indented,
                    });
                }
            }
            WorkspaceListEntry::HiddenHeader { .. } => {
                // Reuse the group-header hit-test path so a click toggles the
                // Hidden section; keyed with a sentinel that no repo can produce.
                headers.push(crate::app::state::GroupHeaderCardArea {
                    name: "Hidden".to_string(),
                    collapse_key: "hidden:".to_string(),
                    rect: Rect::new(body.x, row_y, body.width, 1),
                });
            }
            WorkspaceListEntry::Workspace {
                ws_idx, indented, ..
            } => {
                // Workspace card spans 1 row (name + inline dots).
                cards.push(crate::app::state::WorkspaceCardArea {
                    ws_idx: *ws_idx,
                    rect: Rect::new(body.x, row_y, body.width, 1),
                    indented: *indented,
                });
            }
        }
        row_y = row_y.saturating_add(needed);
    }

    (cards, headers)
}

pub(crate) fn compute_workspace_card_areas(
    app: &AppState,
    area: Rect,
) -> Vec<crate::app::state::WorkspaceCardArea> {
    compute_workspace_list_areas(app, area).0
}

pub(crate) fn workspace_group_chevron_rect(card: &crate::app::state::WorkspaceCardArea) -> Rect {
    if card.rect.width == 0 || card.rect.height == 0 {
        return Rect::default();
    }

    Rect::new(
        card.rect.x + card.rect.width.saturating_sub(1),
        card.rect.y,
        1,
        1,
    )
}

/// Auto-scale sidebar width based on workspace identity + agent summary.
pub(crate) fn collapsed_sidebar_sections(area: Rect) -> (Rect, Option<u16>, Rect) {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height == 0 {
        return (Rect::default(), None, Rect::default());
    }

    if content.height < 7 {
        return (content, None, Rect::default());
    }

    let total_h = content.height as usize;
    let ws_h = total_h.div_ceil(2);
    let detail_h = total_h.saturating_sub(ws_h + 1);
    if ws_h == 0 || detail_h == 0 {
        return (content, None, Rect::default());
    }

    let divider_y = content.y + ws_h as u16;
    let ws_area = Rect::new(content.x, content.y, content.width, ws_h as u16);
    let detail_area = Rect::new(content.x, divider_y + 1, content.width, detail_h as u16);
    (ws_area, Some(divider_y), detail_area)
}

/// Collapsed sidebar: workspace glance on top, compact agent list below.
pub(super) fn render_sidebar_collapsed(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let is_navigating = matches!(app.mode, Mode::Navigate);

    let p = &app.palette;
    frame
        .buffer_mut()
        .set_style(area, Style::default().bg(p.sidebar_bg));
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };
    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    let (ws_area, divider_y, detail_area) = collapsed_sidebar_sections(area);
    if ws_area == Rect::default() {
        render_sidebar_toggle(app, frame, area, true, p);
        return;
    }

    for (visible_idx, ws) in app.workspaces.iter().enumerate() {
        let y = ws_area.y + visible_idx as u16;
        if y >= ws_area.y + ws_area.height {
            break;
        }
        let (agg_state, agg_seen) = ws.aggregate_display_state(&app.terminals);
        let idle_age = ws.oldest_unseen_idle_age(&app.terminals, Instant::now());
        let (icon, icon_style) = state_dot(
            agg_state,
            agg_seen,
            app.spinner_tick,
            app.status_indicators,
            p,
            idle_age,
        );
        let is_selected = visible_idx == app.selected && is_navigating;
        let is_active = Some(visible_idx) == app.active;
        let row_style = if is_selected {
            Style::default().bg(p.selection_bg)
        } else if is_active {
            Style::default().bg(p.active_row_bg)
        } else {
            Style::default()
        };
        let num_style = if is_selected {
            Style::default().fg(p.overlay1).bg(p.selection_bg)
        } else if is_active {
            Style::default().fg(p.text).bg(p.active_row_bg)
        } else {
            Style::default().fg(p.overlay0)
        };

        if is_selected || is_active {
            let buf = frame.buffer_mut();
            for x in ws_area.x..ws_area.x + ws_area.width {
                buf[(x, y)].set_style(row_style);
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<2}", visible_idx + 1), num_style),
                Span::styled(icon, icon_style),
            ])),
            Rect::new(ws_area.x, y, ws_area.width, 1),
        );
    }

    if let Some(divider_y) = divider_y {
        let buf = frame.buffer_mut();
        let divider_color = if app.agent_view_override.is_some() {
            p.accent
        } else {
            p.surface_dim
        };
        for x in ws_area.x..ws_area.x + ws_area.width {
            buf[(x, divider_y)].set_symbol("─");
            buf[(x, divider_y)].set_style(Style::default().fg(divider_color));
        }
    }

    let detail_content_area = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    if detail_content_area != Rect::default() {
        for (detail_idx, detail) in agent_panel_entries(app).iter().enumerate() {
            let y = detail_content_area.y + detail_idx as u16;
            if y >= detail_content_area.y + detail_content_area.height {
                break;
            }
            let position = detail_idx + 1;
            let is_active = app.is_active_pane(detail.ws_idx, detail.tab_idx, detail.pane_id);
            let position_style = if is_active {
                Style::default().fg(p.text).bg(p.active_row_bg)
            } else {
                Style::default().fg(p.overlay0)
            };
            let idle_age = detail
                .idle_since
                .map(|since| Instant::now().saturating_duration_since(since));
            let (icon, icon_style) = agent_icon(
                detail.state,
                detail.seen,
                app.spinner_tick,
                app.status_indicators,
                p,
                idle_age,
            );

            if is_active {
                let buf = frame.buffer_mut();
                for x in detail_content_area.x..detail_content_area.x + detail_content_area.width {
                    buf[(x, y)].set_style(Style::default().bg(p.active_row_bg));
                }
            }
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{position:<2}"), position_style),
                    Span::styled(icon, icon_style),
                ])),
                Rect::new(detail_content_area.x, y, detail_content_area.width, 1),
            );
        }
    }

    render_sidebar_toggle(app, frame, area, true, p);
}

pub(crate) fn workspace_drop_indicator_row(
    cards: &[crate::app::state::WorkspaceCardArea],
    area: Rect,
    insert_idx: usize,
) -> Option<u16> {
    if area.height == 0 {
        return None;
    }
    let list_bottom = area.y + area.height.saturating_sub(1);

    let first = cards.first()?;
    if insert_idx == first.ws_idx {
        return first.rect.y.checked_sub(1).filter(|y| *y < list_bottom);
    }

    if let Some(row) = cards
        .last()
        .filter(|card| insert_idx == card.ws_idx.saturating_add(1))
        .map(|card| card.rect.y.saturating_add(card.rect.height))
        .filter(|y| *y < list_bottom)
    {
        return Some(row);
    }

    if let Some(card) = cards.iter().find(|card| card.ws_idx == insert_idx) {
        return card.rect.y.checked_sub(1).filter(|y| *y < list_bottom);
    }

    None
}

pub(super) fn render_sidebar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let p = &app.palette;
    frame
        .buffer_mut()
        .set_style(area, Style::default().bg(p.sidebar_bg));
    let is_navigating = matches!(app.mode, Mode::Navigate);
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };

    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    let (ws_area, detail_area) = expanded_sidebar_sections(area, app.sidebar_section_split);

    render_workspace_list(app, terminal_runtimes, frame, ws_area, is_navigating);
    render_programs_section(app, frame, sidebar_programs_band_rect(app, ws_area));
    render_agent_detail(app, terminal_runtimes, frame, detail_area);
    render_sidebar_toggle(app, frame, area, false, p);
}

/// Sidebar "Programs" launcher band: one row per pane-mode `.bora.toml`
/// command for the active workspace, plus a fixed "+ run command…" row.
/// Row count/geometry MUST match `sidebar_programs_band_rect` exactly.
fn render_programs_section(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    let commands = app.sidebar_program_commands();
    let mut row_y = area.y;
    let bottom = area.y + area.height;
    for cmd in &commands {
        if row_y >= bottom {
            return;
        }
        let label = truncate_end(&cmd.label, (area.width as usize).saturating_sub(1));
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {label}"),
                Style::default().fg(p.text),
            )),
            Rect::new(area.x, row_y, area.width, 1),
        );
        row_y = row_y.saturating_add(1);
    }
    if row_y < bottom {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " + run command…",
                Style::default().fg(p.overlay0),
            )),
            Rect::new(area.x, row_y, area.width, 1),
        );
    }
}
fn render_workspace_list(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
    is_navigating: bool,
) {
    let p = &app.palette;
    let dragged_ws_idx = match app.drag.as_ref().map(|drag| &drag.target) {
        Some(crate::app::state::DragTarget::WorkspaceReorder { source_ws_idx, .. }) => {
            Some(*source_ws_idx)
        }
        _ => None,
    };
    let insertion_row = match app.drag.as_ref().map(|drag| &drag.target) {
        Some(crate::app::state::DragTarget::WorkspaceReorder {
            insert_idx: Some(insert_idx),
            ..
        }) => workspace_drop_indicator_row(&app.view.workspace_card_areas, area, *insert_idx),
        _ => None,
    };

    let list_bottom = area.y + area.height.saturating_sub(1);
    if area.height > 0 {
        let header_line = Line::from(vec![Span::styled(
            " spaces",
            Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
        )]);
        frame.render_widget(
            Paragraph::new(header_line),
            Rect::new(area.x, area.y, area.width, 1),
        );
    }

    let metrics = workspace_list_scroll_metrics(app, area);
    let scrollbar_rect = workspace_list_scrollbar_rect(app, area);

    // --- Render entries using the same lockstep iteration ---
    let entries = workspace_list_entries(app);
    let scroll = app.workspace_scroll;
    let body = workspace_list_body_rect(app, area, scrollbar_rect.is_some());
    let mut row_y = body.y;
    let now = Instant::now();

    for (entry_idx, entry) in entries.iter().enumerate().skip(scroll) {
        let needed = entry_row_height(entry, &entries, entry_idx);
        if row_y.saturating_add(needed) > body.y + body.height {
            break;
        }
        match entry {
            WorkspaceListEntry::GroupHeader { name, collapse_key } => {
                if row_y < list_bottom {
                    let collapsed = app.collapsed_space_keys.contains(collapse_key);
                    let chevron = if collapsed { "▸" } else { "▾" };
                    let mut spans = vec![
                        Span::styled(chevron, Style::default().fg(p.accent)),
                        Span::styled(" ", Style::default()),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
                        ),
                    ];
                    if collapsed && !collapse_key.starts_with("vg:") {
                        let (state, seen) = space_aggregate_display_state(app, collapse_key);
                        let age = space_aggregate_idle_age(app, collapse_key, now);
                        let (dot, dot_style) =
                            state_dot(state, seen, app.spinner_tick, app.status_indicators, p, age);
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(dot, dot_style));
                        if let Some(age) = age {
                            spans.push(Span::styled(
                                format!(" {}", format_idle_age(age)),
                                Style::default().fg(idle_age_color(Some(age), p)),
                            ));
                        }
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                    if app.mouse_capture
                        && body.width >= 3
                        && !collapse_key.starts_with("vg:")
                        && !collapse_key.starts_with("prs:")
                    {
                        frame.render_widget(
                            Paragraph::new(" + ").style(Style::default().fg(p.overlay1)),
                            Rect::new(body.x + body.width - 3, row_y, 3, 1),
                        );
                    }
                }
            }
            WorkspaceListEntry::ProjectHeader {
                name,
                collapse_key,
                indented,
                branch,
            } => {
                if row_y < list_bottom {
                    let collapsed = app.collapsed_space_keys.contains(collapse_key);
                    let name_style = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
                    let mut spans = if *indented {
                        // Nested under a visual group: legacy plain label.
                        vec![Span::styled(format!(" {name}"), name_style)]
                    } else {
                        // Top-level: open the rounded bracket rail.
                        vec![
                            Span::styled("╭─", Style::default().fg(p.overlay0)),
                            Span::styled(name.clone(), name_style),
                        ]
                    };
                    if let Some(b) = branch {
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(
                            format!("[{}]", b.label),
                            Style::default().fg(p.overlay1),
                        ));
                        if b.ahead > 0 {
                            spans.push(Span::styled(" ", Style::default()));
                            spans.push(Span::styled(
                                format!("↑{}", b.ahead),
                                Style::default().fg(p.green),
                            ));
                        }
                        if b.behind > 0 {
                            spans.push(Span::styled(" ", Style::default()));
                            spans.push(Span::styled(
                                format!("↓{}", b.behind),
                                Style::default().fg(p.red),
                            ));
                        }
                        // PR badge for the folded first-branch workspace (the
                        // next entry), mirroring the branch-header badge.
                        if let Some(WorkspaceListEntry::Workspace { ws_idx, .. }) =
                            entries.get(entry_idx + 1)
                        {
                            if let Some(cs) = app
                                .workspaces
                                .get(*ws_idx)
                                .and_then(|w| w.cached_check_status.as_ref())
                            {
                                if let Some(pr) = cs.pr.as_ref() {
                                    let pr_color = match pr.state.as_str() {
                                        "MERGED" => p.mauve,
                                        "CLOSED" => p.red,
                                        _ => p.green,
                                    };
                                    spans.push(Span::styled(" ", Style::default()));
                                    spans.push(Span::styled(
                                        format!("#{}", pr.number),
                                        Style::default().fg(pr_color),
                                    ));
                                    if let Some((glyph, style)) = checks_badge(&cs.checks, p) {
                                        spans.push(Span::styled(glyph, style));
                                    }
                                }
                            }
                        }
                    }
                    if collapsed {
                        let (state, seen) = space_aggregate_display_state(app, collapse_key);
                        let age = space_aggregate_idle_age(app, collapse_key, now);
                        let (dot, dot_style) =
                            state_dot(state, seen, app.spinner_tick, app.status_indicators, p, age);
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(dot, dot_style));
                        if let Some(age) = age {
                            spans.push(Span::styled(
                                format!(" {}", format_idle_age(age)),
                                Style::default().fg(idle_age_color(Some(age), p)),
                            ));
                        }
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                    if app.mouse_capture && body.width >= 3 {
                        frame.render_widget(
                            Paragraph::new(" + ").style(Style::default().fg(p.overlay1)),
                            Rect::new(body.x + body.width - 3, row_y, 3, 1),
                        );
                    }
                }
            }
            WorkspaceListEntry::BranchHeader {
                label,
                ahead,
                behind,
                indented,
                last,
                ws_idx,
            } => {
                if row_y < list_bottom {
                    let indent = if *indented { " " } else { "" };
                    // All connectors are 4 cells wide so branch labels align
                    // across mid (├──), last (╰──), and nested rows.
                    let connector = if *last { "╰── " } else { "├── " };
                    let mut spans = vec![Span::styled(
                        format!("{indent}{connector}"),
                        Style::default().fg(p.overlay0),
                    )];

                    // `Some` means this header folded its sole workspace's
                    // row into itself: draw it (dot, idle age, selection
                    // highlight) the way the `Workspace` arm does, with the
                    // branch label standing in for the name.
                    let collapsed_ws = ws_idx.map(|idx| &app.workspaces[idx]);
                    let highlighted = ws_idx.is_some_and(|idx| {
                        (idx == app.selected && is_navigating)
                            || Some(idx) == app.active
                            || dragged_ws_idx == Some(idx)
                    });
                    if let Some(idx) = *ws_idx {
                        if highlighted {
                            let selected = idx == app.selected && is_navigating;
                            let bg = if selected {
                                p.selection_bg
                            } else if dragged_ws_idx == Some(idx) {
                                p.surface1
                            } else {
                                p.active_row_bg
                            };
                            let buf = frame.buffer_mut();
                            for x in body.x..body.x + body.width {
                                buf[(x, row_y)].set_style(Style::default().bg(bg));
                            }
                        }
                    }
                    let name_style = if highlighted {
                        Style::default().fg(p.text).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(p.overlay1)
                    };
                    if let Some(ws) = collapsed_ws {
                        let dots = tab_dot_states(ws, &app.terminals);
                        let dot_ages = tab_dot_idle_ages(ws, &app.terminals, now);
                        for (tab_idx, &(state, seen)) in dots.iter().enumerate() {
                            let (dot_glyph, mut dot_style) = state_dot(
                                state,
                                seen,
                                app.spinner_tick,
                                app.status_indicators,
                                p,
                                dot_ages.get(tab_idx).copied().flatten(),
                            );
                            if tab_idx == ws.active_tab {
                                dot_style = dot_style.add_modifier(Modifier::BOLD);
                            }
                            if tab_idx > 0 {
                                spans.push(Span::styled(" ", Style::default()));
                            }
                            spans.push(Span::styled(dot_glyph, dot_style));
                        }
                        if !dots.is_empty() {
                            spans.push(Span::styled(" ", Style::default()));
                        }
                    }
                    spans.push(Span::styled(label.clone(), name_style));
                    if *ahead > 0 {
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(
                            format!("↑{ahead}"),
                            Style::default().fg(p.green),
                        ));
                    }
                    if *behind > 0 {
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(
                            format!("↓{behind}"),
                            Style::default().fg(p.red),
                        ));
                    }
                    if let Some(ws) = collapsed_ws {
                        let idle_age = ws
                            .oldest_unseen_idle_age(&app.terminals, now)
                            .or_else(|| ws.oldest_idle_age(&app.terminals, now));
                        if let Some(age) = idle_age {
                            spans.push(Span::styled(
                                format!(" {}", format_idle_age(age)),
                                Style::default().fg(idle_age_color(Some(age), p)),
                            ));
                        }
                    } else if let Some(WorkspaceListEntry::Workspace { ws_idx, .. }) =
                        entries.get(entry_idx + 1)
                    {
                        if let Some(cs) = app
                            .workspaces
                            .get(*ws_idx)
                            .and_then(|w| w.cached_check_status.as_ref())
                        {
                            if let Some(pr) = cs.pr.as_ref() {
                                let pr_color = match pr.state.as_str() {
                                    "MERGED" => p.mauve,
                                    "CLOSED" => p.red,
                                    _ => p.green,
                                };
                                spans.push(Span::styled(" ", Style::default()));
                                spans.push(Span::styled(
                                    format!("#{}", pr.number),
                                    Style::default().fg(pr_color),
                                ));
                                if let Some((glyph, style)) = checks_badge(&cs.checks, p) {
                                    spans.push(Span::styled(glyph, style));
                                }
                            }
                        }
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::HiddenHeader { count } => {
                if row_y < list_bottom {
                    let chevron = if app.hidden_section_expanded {
                        "▾"
                    } else {
                        "▸"
                    };
                    let spans = vec![
                        Span::styled(chevron, Style::default().fg(p.overlay0)),
                        Span::styled(" ", Style::default()),
                        Span::styled(
                            format!("Hidden ({count})"),
                            Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
                        ),
                    ];
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::Workspace {
                ws_idx,
                indented,
                rail,
            } => {
                let i = *ws_idx;
                let ws = &app.workspaces[i];
                let selected = i == app.selected && is_navigating;
                let is_active = Some(i) == app.active;
                let is_dragged = dragged_ws_idx == Some(i);
                let highlighted = selected || is_active || is_dragged;

                // Card rect spans 1 row (name + inline dots).
                let card_height = 1u16;
                if highlighted {
                    let bg = if selected {
                        p.selection_bg
                    } else if is_dragged {
                        p.surface1
                    } else {
                        p.active_row_bg
                    };
                    let buf = frame.buffer_mut();
                    for y in row_y..row_y + card_height {
                        if y >= list_bottom {
                            break;
                        }
                        for x in body.x..body.x + body.width {
                            buf[(x, y)].set_style(Style::default().bg(bg));
                        }
                    }
                }

                let name_style = if highlighted {
                    Style::default().fg(p.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p.subtext0)
                };
                let rail_style = Style::default().fg(p.overlay0);

                // --- Single row: name + inline tab dots ---
                let mut line1 = Vec::new();
                let indent_prefix = if *indented { " " } else { "" };
                match rail {
                    BranchRail::Spine => {
                        // Bracket rails anchor at column 0 under the header's `╭─`.
                        // All bracket rails are 4 cells wide so workspace names
                        // align across │ / ╰── / blank rows.
                        line1.push(Span::styled("│   ", rail_style));
                    }
                    BranchRail::Close => {
                        line1.push(Span::styled("╰── ", rail_style));
                    }
                    BranchRail::None => {
                        if let Some((key, collapsed)) =
                            workspace_parent_group_state(app, i).filter(|_| !*indented)
                        {
                            let chevron = if collapsed { "▸" } else { "▾" };
                            line1.push(Span::styled(chevron, Style::default().fg(p.accent)));
                            if collapsed {
                                let (state, seen) = space_aggregate_display_state(app, &key);
                                let age = space_aggregate_idle_age(app, &key, now);
                                let (si, ss) = state_dot(
                                    state,
                                    seen,
                                    app.spinner_tick,
                                    app.status_indicators,
                                    p,
                                    age,
                                );
                                line1.push(Span::styled(" ", Style::default()));
                                line1.push(Span::styled(si, ss));
                                if let Some(age) = age {
                                    line1.push(Span::styled(
                                        format!(" {}", format_idle_age(age)),
                                        Style::default().fg(idle_age_color(Some(age), p)),
                                    ));
                                }
                            }
                            line1.push(Span::styled(" ", Style::default()));
                        } else {
                            line1.push(Span::styled(indent_prefix, Style::default()));
                        }
                    }
                }

                // Build the tab dots, placed to the LEFT of the name on this
                // same row. Preserve the existing glyph/style logic verbatim;
                // one space separates each dot, and one space separates the
                // dots group from the name.
                let dots = tab_dot_states(ws, &app.terminals);
                let dot_ages = tab_dot_idle_ages(ws, &app.terminals, now);
                let mut dot_spans: Vec<Span> = Vec::new();
                for (tab_idx, &(state, seen)) in dots.iter().enumerate() {
                    let (dot_glyph, mut dot_style) = state_dot(
                        state,
                        seen,
                        app.spinner_tick,
                        app.status_indicators,
                        p,
                        dot_ages.get(tab_idx).copied().flatten(),
                    );
                    if tab_idx == ws.active_tab {
                        dot_style = dot_style.add_modifier(Modifier::BOLD);
                    }
                    if tab_idx > 0 {
                        dot_spans.push(Span::styled(" ", Style::default()));
                    }
                    dot_spans.push(Span::styled(dot_glyph, dot_style));
                }
                // One space between the dots group and the name.
                let sep = if dot_spans.is_empty() { "" } else { " " };

                // Truncate the name so the dots + separator still fit.
                let prefix_width: usize = line1
                    .iter()
                    .map(|s| display_width(s.content.as_ref()))
                    .sum();
                let dots_width: usize = dot_spans
                    .iter()
                    .map(|s| display_width(s.content.as_ref()))
                    .sum();
                // Idle time follows the same age color ramp whether the idle
                // pane was already seen or not.
                let idle_age = ws
                    .oldest_unseen_idle_age(&app.terminals, now)
                    .or_else(|| ws.oldest_idle_age(&app.terminals, now));
                let idle_color = idle_age_color(idle_age, p);
                let idle_suffix = idle_age.map(|age| format!(" {}", format_idle_age(age)));
                let idle_width = idle_suffix
                    .as_deref()
                    .map(display_width)
                    .unwrap_or_default();
                // Metadata tokens reported through `bora workspace
                // report-metadata` (a channel's unread badge, a `$pr` chip).
                //
                // With the default `[ui.sidebar.spaces] rows` (state_icon,
                // workspace, branch, git_status — none of them custom), only
                // reported metadata values are drawn here: the state dot,
                // name, branch and git status are already hand-drawn above,
                // so painting the whole resolved row would repeat them, and
                // when the config names no custom token every reported value
                // is drawn instead so a badge is visible without the reporter
                // and the reader having to agree on a key in config first.
                //
                // Once the reader names at least one custom token in a row,
                // they've opted into full control over that row's layout:
                // every configured token (state icon aside, which is always
                // shown via the tab dots) draws in the order and style they
                // wrote, even if that repeats the workspace name.
                let full_label = ws.display_name_from(&app.terminals, terminal_runtimes);
                let token_spans: Vec<Span<'static>> = if ws.metadata_tokens.is_empty() {
                    Vec::new()
                } else {
                    let (row_state, row_seen) = ws.aggregate_state(&app.terminals);
                    let branch = ws.branch();
                    let token_values = ws.metadata_tokens.values();
                    let rows = tokens::space_rows(
                        &app.sidebar_spaces,
                        SpaceTokenContext {
                            workspace: &full_label,
                            branch: branch.as_deref(),
                            state_text: state_label(row_state, row_seen),
                            ahead_behind: ws.git_ahead_behind(),
                            tokens: &token_values,
                            suppress_git_details: *indented,
                        },
                    );
                    let mut customs: Vec<ResolvedToken> = rows
                        .iter()
                        .flatten()
                        .filter(|token| matches!(token.kind, ResolvedTokenKind::Custom(_)))
                        .cloned()
                        .collect();
                    if customs.is_empty() {
                        let mut values: Vec<(&String, &String)> = token_values.iter().collect();
                        values.sort_unstable_by_key(|(key, _)| *key);
                        customs = values
                            .into_iter()
                            .map(|(_, value)| ResolvedToken {
                                kind: ResolvedTokenKind::Custom(value.clone()),
                                style: crate::config::SidebarTokenStyle::default(),
                            })
                            .collect();
                    } else {
                        customs = rows
                            .into_iter()
                            .flatten()
                            .filter(|token| !matches!(token.kind, ResolvedTokenKind::StateIcon))
                            .collect();
                    }
                    if customs.is_empty() {
                        Vec::new()
                    } else {
                        let state_icon = state_dot(
                            row_state,
                            row_seen,
                            app.spinner_tick,
                            app.status_indicators,
                            p,
                            idle_age,
                        );
                        let state_text_style = Style::default()
                            .fg(state_label_color(row_state, row_seen, p))
                            .add_modifier(Modifier::DIM);
                        let branch_style =
                            Style::default().fg(if highlighted { p.mauve } else { p.overlay0 });
                        resolved_token_spans(
                            &customs,
                            state_icon,
                            state_text_style,
                            name_style,
                            branch_style,
                            branch_style,
                            p,
                            (body.width as usize).saturating_sub(
                                prefix_width + dots_width + display_width(sep) + idle_width,
                            ),
                        )
                    }
                };
                let token_width: usize = if token_spans.is_empty() {
                    0
                } else {
                    1 + token_spans
                        .iter()
                        .map(|s| display_width(s.content.as_ref()))
                        .sum::<usize>()
                };
                // Identity badges: registered/detected agent name, joined
                // `#`-channels, and the "safe to close" collectible mark.
                // Purely in-memory or already-cached lookups — nothing here
                // touches disk.
                let agent_suffix =
                    workspace_agent_label(ws, &app.terminals).map(|name| format!(" @{name}"));
                let agent_width = agent_suffix.as_deref().map(display_width).unwrap_or(0);
                let channel_suffix = match ws.cached_channels.split_first() {
                    Some((first, [])) => Some(format!(" #{first}")),
                    Some((first, rest)) => Some(format!(" #{first} +{}", rest.len())),
                    None => None,
                };
                let channel_width = channel_suffix.as_deref().map(display_width).unwrap_or(0);
                let collectible_suffix = (ws.cached_collectible == Some(true)).then_some(" ✓");
                let collectible_width = collectible_suffix.map(display_width).unwrap_or(0);
                let avail = (body.width as usize).saturating_sub(
                    prefix_width
                        + dots_width
                        + display_width(sep)
                        + idle_width
                        + token_width
                        + agent_width
                        + channel_width
                        + collectible_width,
                );
                let label = truncate_end(&full_label, avail);
                line1.extend(dot_spans);
                line1.push(Span::styled(sep, Style::default()));
                line1.push(Span::styled(label, name_style));
                if let Some(agent) = agent_suffix {
                    line1.push(Span::styled(agent, Style::default().fg(p.mauve)));
                }
                if let Some(channel) = channel_suffix {
                    line1.push(Span::styled(channel, Style::default().fg(p.teal)));
                }
                if let Some(marker) = collectible_suffix {
                    line1.push(Span::styled(
                        marker,
                        Style::default().fg(p.overlay0).add_modifier(Modifier::DIM),
                    ));
                }
                if !token_spans.is_empty() {
                    line1.push(Span::raw(" "));
                    line1.extend(token_spans);
                }
                if let Some(suffix) = idle_suffix {
                    line1.push(Span::styled(suffix, Style::default().fg(idle_color)));
                }

                if row_y < list_bottom {
                    frame.render_widget(
                        Paragraph::new(Line::from(line1)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
        }
        row_y = row_y.saturating_add(needed);
    }

    if let Some(y) = insertion_row.filter(|y| *y < list_bottom) {
        let indicator_right = scrollbar_rect
            .map(|rect| rect.x)
            .unwrap_or(area.x + area.width);
        let buf = frame.buffer_mut();
        for x in area.x..indicator_right {
            buf[(x, y)].set_symbol("─");
            buf[(x, y)].set_style(Style::default().fg(p.accent));
        }
    }

    if let Some(track) = scrollbar_rect {
        render_scrollbar(frame, metrics, track, p.surface_dim, p.overlay0, "▕");
    }

    if app.mouse_capture && list_bottom > area.y {
        let new_rect = app.sidebar_new_button_rect();
        frame.render_widget(
            Paragraph::new(Span::styled(" new", Style::default().fg(p.overlay0))),
            new_rect,
        );

        let menu_rect = app.global_launcher_rect();
        let menu_line = if app.global_menu_attention_badge_visible() {
            Line::from(vec![
                Span::styled(
                    "● ",
                    Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled("menu", Style::default().fg(p.overlay0)),
            ])
        } else {
            Line::from(vec![Span::styled("menu", Style::default().fg(p.overlay0))])
        };
        frame.render_widget(
            Paragraph::new(menu_line).alignment(Alignment::Right),
            menu_rect,
        );
    }
}

fn render_agent_detail(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let p = &app.palette;

    if area.height < 3 {
        return;
    }

    let sep_line = "─".repeat(area.width as usize);
    frame.render_widget(
        Paragraph::new(Span::styled(&sep_line, Style::default().fg(p.surface_dim))),
        Rect::new(area.x, area.y, area.width, 1),
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            " agents",
            Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
        )])),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
    let control_label = active_agent_view_label(app)
        .unwrap_or_else(|| agent_panel_sort_label(app.agent_panel_sort));
    let toggle_rect = agent_panel_header_label_rect(area, control_label);
    if toggle_rect != Rect::default() {
        let color = if app.agent_view_override.is_some() {
            p.accent
        } else {
            p.overlay0
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                control_label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
            toggle_rect,
        );
    }

    let details = agent_panel_entries_from(app, terminal_runtimes);
    let metrics = agent_panel_scroll_metrics(app, area);
    let scrollbar_rect = agent_panel_scrollbar_rect(app, area);
    let body = agent_panel_body_rect(area, should_show_scrollbar(metrics));
    if body == Rect::default() {
        return;
    }
    if details.is_empty() && app.agent_view_override.is_some() {
        frame.render_widget(
            Paragraph::new(" no matching agents")
                .style(Style::default().fg(p.overlay0).add_modifier(Modifier::DIM)),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    }

    let scroll = app.agent_panel_scroll.min(metrics.max_offset_from_bottom);
    let mut row_y = body.y;
    let body_bottom = body.y + body.height;
    for (index, detail) in details.iter().enumerate().skip(scroll) {
        let rows = resolved_agent_rows(app, detail);
        let height = (rows.len().max(1) as u16).min(body.height);
        if row_y.saturating_add(height) > body_bottom {
            break;
        }

        // Check if this agent entry corresponds to the active session
        let is_active = app.is_active_pane(detail.ws_idx, detail.tab_idx, detail.pane_id);

        let idle_age = detail
            .idle_since
            .map(|since| Instant::now().saturating_duration_since(since));
        let icon = agent_icon(
            detail.state,
            detail.seen,
            app.spinner_tick,
            app.status_indicators,
            p,
            idle_age,
        );
        let label_color = state_label_color(detail.state, detail.seen, p);

        let row_style = if is_active {
            Style::default().bg(p.active_row_bg)
        } else {
            Style::default()
        };
        let name_style = if is_active {
            Style::default().fg(p.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.subtext0).add_modifier(Modifier::BOLD)
        };
        let status_style = if is_active {
            Style::default().fg(label_color)
        } else {
            Style::default().fg(label_color).add_modifier(Modifier::DIM)
        };
        let agent_style = Style::default().fg(p.overlay0).add_modifier(Modifier::DIM);

        let row_count = height as usize;
        for (row_index, resolved) in rows.iter().take(row_count).enumerate() {
            let mut spans = vec![Span::raw(if row_index == 0 { " " } else { "   " })];
            spans.extend(resolved_token_spans(
                resolved,
                icon,
                status_style,
                name_style,
                agent_style,
                agent_style,
                p,
                body.width
                    .saturating_sub(if row_index == 0 { 1 } else { 3 }) as usize,
            ));
            // `custom_status` (set via `pane.report_metadata`) has no token
            // representation in `[ui.sidebar.agents] rows`, so it always
            // rides on the entry's last visible row instead.
            if row_index + 1 == row_count {
                if let Some(custom_status) = &detail.custom_status {
                    spans.push(Span::styled(" · ", agent_style));
                    spans.push(Span::styled(custom_status.clone(), agent_style));
                }
            }
            frame.render_widget(
                Paragraph::new(Line::from(spans)).style(row_style),
                Rect::new(body.x, row_y + row_index as u16, body.width, 1),
            );
        }
        row_y = row_y
            .saturating_add(height)
            .saturating_add(agent_entry_gap(app, index, details.len()))
            .min(body_bottom);
    }

    if let Some(track) = scrollbar_rect {
        render_scrollbar(frame, metrics, track, p.surface_dim, p.overlay0, "▕");
    }
}

pub(crate) fn collapsed_sidebar_toggle_rect(area: Rect) -> Rect {
    let bottom_y = area.y + area.height.saturating_sub(1);
    let content_w = area.width.saturating_sub(1);
    if content_w == 0 || area.height == 0 {
        return Rect::default();
    }
    let x = area.x + content_w / 2;
    Rect::new(x, bottom_y, 1, 1)
}

pub(crate) fn expanded_sidebar_toggle_rect(area: Rect) -> Rect {
    if area.width <= 1 || area.height == 0 {
        return Rect::default();
    }
    Rect::new(
        area.x + area.width.saturating_sub(2),
        area.y + area.height.saturating_sub(1),
        1,
        1,
    )
}

fn render_sidebar_toggle(
    app: &AppState,
    frame: &mut Frame,
    area: Rect,
    collapsed: bool,
    p: &Palette,
) {
    let toggle_area = if collapsed {
        collapsed_sidebar_toggle_rect(area)
    } else {
        expanded_sidebar_toggle_rect(area)
    };
    if toggle_area == Rect::default() {
        return;
    }
    let icon = if collapsed { "»" } else { "«" };
    let icon_style = if collapsed && app.global_menu_attention_badge_visible() {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    frame.render_widget(Paragraph::new(Span::styled(icon, icon_style)), toggle_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{detect::Agent, layout::PaneId, workspace::Workspace};
    use ratatui::{backend::TestBackend, layout::Direction, Terminal};

    fn row_text(buffer: &ratatui::buffer::Buffer, row: u16, width: u16) -> String {
        (0..width)
            .map(|x| buffer[(x, row)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn find_symbol_x(buffer: &ratatui::buffer::Buffer, row: u16, width: u16, symbol: &str) -> u16 {
        (0..width)
            .find(|x| buffer[(*x, row)].symbol() == symbol)
            .unwrap_or_else(|| {
                panic!(
                    "missing symbol {symbol:?} in row {}",
                    row_text(buffer, row, width)
                )
            })
    }

    #[test]
    fn expanded_and_collapsed_sidebars_use_custom_background() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces.clear();
        app.active = None;
        app.palette.sidebar_bg = ratatui::style::Color::Rgb(12, 34, 56);
        let area = Rect::new(0, 0, 26, 20);

        let mut expanded = Terminal::new(TestBackend::new(26, 20)).unwrap();
        expanded
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        assert!(expanded
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.bg == app.palette.sidebar_bg));

        let mut collapsed = Terminal::new(TestBackend::new(26, 20)).unwrap();
        collapsed
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .unwrap();
        assert!(collapsed
            .backend()
            .buffer()
            .content
            .iter()
            .all(|cell| cell.bg == app.palette.sidebar_bg));
    }

    #[test]
    fn workspace_agent_label_prefers_registered_name_over_detected_label() {
        let mut app = crate::app::state::AppState::test_new();
        let workspace = Workspace::test_new("bridge");
        let first_pane = workspace.tabs[0].root_pane;
        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(Agent::Pi);

        // Detected only: falls back to the detected agent's label.
        let detected_only = workspace_agent_label(&app.workspaces[0], &app.terminals);
        assert_eq!(detected_only.as_deref(), Some("pi"));

        // A registered `agent rename` name wins over the detected label.
        app.terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_agent_name("planner".into());
        let registered = workspace_agent_label(&app.workspaces[0], &app.terminals);
        assert_eq!(registered.as_deref(), Some("planner"));
    }

    #[test]
    fn workspace_row_renders_agent_channel_and_collectible_badges() {
        let mut app = crate::app::state::AppState::test_new();
        let workspace = Workspace::test_new("worktree-branch");
        let root_pane = workspace.tabs[0].root_pane;
        app.workspaces = vec![workspace];
        app.group_workspaces_by_repo = false;
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].tabs[0].panes[&root_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_agent_name("planner".into());
        app.workspaces[0].cached_channels = vec!["eng".into()];
        app.workspaces[0].cached_collectible = Some(true);
        app.active = Some(0);

        let area = Rect::new(0, 0, 60, 10);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let full_text: String = (0..area.height)
            .map(|row| row_text(buffer, row, area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            full_text.contains("@planner"),
            "row should show registered agent name: {full_text:?}"
        );
        assert!(
            full_text.contains("#eng"),
            "row should show joined channel: {full_text:?}"
        );
        assert!(
            full_text.contains('✓'),
            "row should show collectible marker: {full_text:?}"
        );
    }

    #[test]
    fn default_agent_rows_remove_redundant_state_text() {
        let mut app = crate::app::state::AppState::test_new();
        let workspace = Workspace::test_new("one");
        let pane_id = workspace.tabs[0].root_pane;
        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        app.active = Some(0);
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal_state = app.terminals.get_mut(&terminal_id).unwrap();
        terminal_state.detected_agent = Some(Agent::Pi);
        terminal_state.state = AgentState::Working;

        let area = Rect::new(0, 0, 26, 20);
        let mut terminal = Terminal::new(TestBackend::new(26, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (_, agent_area) = expanded_sidebar_sections(area, app.sidebar_section_split);
        let body = agent_panel_body_rect(agent_area, false);

        let first = row_text(buffer, body.y, 25);
        let second = row_text(buffer, body.y + 1, 25);
        assert!(first.contains("one"));
        assert_eq!(second, "   pi");
        assert!(!first.contains("working"));
        assert!(!second.contains("working"));

        let workspace_x = find_symbol_x(buffer, body.y, body.width, "o");
        let workspace_style = buffer[(workspace_x, body.y)].style();
        assert_eq!(workspace_style.fg, Some(app.palette.text));
        assert!(workspace_style.add_modifier.contains(Modifier::BOLD));
        assert!(!workspace_style.add_modifier.contains(Modifier::DIM));
        assert_eq!(workspace_style.bg, Some(app.palette.active_row_bg));

        let agent_x = find_symbol_x(buffer, body.y + 1, body.width, "p");
        let agent_style = buffer[(agent_x, body.y + 1)].style();
        assert_eq!(agent_style.fg, Some(app.palette.overlay0));
        assert!(agent_style.add_modifier.contains(Modifier::DIM));
        assert!(!agent_style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(agent_style.bg, Some(app.palette.active_row_bg));
    }

    #[test]
    fn occurrence_false_removes_default_workspace_bold_and_agent_dim() {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.agents]
rows = [[{ token = "workspace", bold = false }, { token = "agent", dim = false }]]
"##,
        )
        .unwrap();
        let mut app = crate::app::state::AppState::test_new();
        app.sidebar_agents = config.ui.sidebar.agents;
        let workspace = Workspace::test_new("one");
        let pane_id = workspace.tabs[0].root_pane;
        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        app.active = Some(0);
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(Agent::Pi);

        let area = Rect::new(0, 0, 26, 20);
        let mut terminal = Terminal::new(TestBackend::new(26, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let (_, agent_area) = expanded_sidebar_sections(area, app.sidebar_section_split);
        let body = agent_panel_body_rect(agent_area, false);
        let buffer = terminal.backend().buffer();
        let workspace = buffer[(find_symbol_x(buffer, body.y, body.width, "o"), body.y)].style();
        let agent = buffer[(find_symbol_x(buffer, body.y, body.width, "p"), body.y)].style();

        assert_eq!(workspace.fg, Some(app.palette.text));
        assert!(!workspace.add_modifier.contains(Modifier::BOLD));
        assert_eq!(agent.fg, Some(app.palette.overlay0));
        assert!(!agent.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn default_space_workspace_style_tracks_active_state() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.active = Some(0);
        app.mode = Mode::Terminal;
        let area = Rect::new(0, 0, 26, 20);
        app.view.workspace_card_areas = compute_workspace_card_areas(&app, area);
        let first_row = app.view.workspace_card_areas[0].rect.y;
        let second_row = app.view.workspace_card_areas[1].rect.y;
        let mut terminal = Terminal::new(TestBackend::new(26, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();

        let active = buffer[(find_symbol_x(buffer, first_row, 25, "o"), first_row)].style();
        assert_eq!(active.fg, Some(app.palette.text));
        assert!(active.add_modifier.contains(Modifier::BOLD));
        assert!(!active.add_modifier.contains(Modifier::DIM));
        assert_eq!(active.bg, Some(app.palette.active_row_bg));

        let inactive = buffer[(find_symbol_x(buffer, second_row, 25, "t"), second_row)].style();
        assert_eq!(inactive.fg, Some(app.palette.subtext0));
        assert!(!inactive
            .add_modifier
            .intersects(Modifier::BOLD | Modifier::DIM));
        assert_eq!(inactive.bg, Some(ratatui::style::Color::Reset));
    }

    #[test]
    fn navigate_selection_keeps_its_existing_background_beside_active_workspace() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.active = Some(0);
        app.selected = 1;
        app.mode = Mode::Navigate;
        let area = Rect::new(0, 0, 26, 20);
        app.view.workspace_card_areas = compute_workspace_card_areas(&app, area);
        let active_row = app.view.workspace_card_areas[0].rect.y;
        let selected_row = app.view.workspace_card_areas[1].rect.y;
        let mut terminal = Terminal::new(TestBackend::new(26, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(
            buffer[(0, active_row)].bg,
            app.palette.active_row_bg,
            "active workspace should keep its dedicated background"
        );
        assert_eq!(
            buffer[(0, selected_row)].bg,
            app.palette.selection_bg,
            "navigate selection should use its dedicated cursor background"
        );
    }

    #[test]
    fn space_occurrence_style_applies_without_styling_separator() {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.spaces]
rows = [[{ token = "$hype", fg = "#abcdef", bold = true, dim = false }, "workspace"]]
"##,
        )
        .unwrap();
        let mut app = crate::app::state::AppState::test_new();
        app.sidebar_spaces = config.ui.sidebar.spaces;
        app.workspaces = vec![Workspace::test_new("one")];
        app.active = Some(0);
        app.mode = Mode::Terminal;
        app.workspaces[0].metadata_tokens.patch(
            std::collections::HashMap::from([("hype".into(), Some("HI".into()))]),
            None,
            std::time::Instant::now(),
        );

        let area = Rect::new(0, 0, 26, 20);
        app.view.workspace_card_areas = compute_workspace_card_areas(&app, area);
        let row = app.view.workspace_card_areas[0].rect.y;
        let mut terminal = Terminal::new(TestBackend::new(26, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let h = buffer[(find_symbol_x(buffer, row, 25, "H"), row)].style();
        let i = buffer[(find_symbol_x(buffer, row, 25, "I"), row)].style();
        let separator = buffer[(find_symbol_x(buffer, row, 25, "·"), row)].style();

        for style in [h, i] {
            assert_eq!(style.fg, Some(ratatui::style::Color::Rgb(0xab, 0xcd, 0xef)));
            assert!(style.add_modifier.contains(Modifier::BOLD));
            assert!(!style.add_modifier.contains(Modifier::DIM));
            assert_eq!(style.bg, Some(app.palette.active_row_bg));
        }
        assert_eq!(separator.fg, Some(app.palette.overlay0));
        assert!(separator.add_modifier.contains(Modifier::DIM));
        assert!(!separator.add_modifier.contains(Modifier::BOLD));
        assert_eq!(separator.bg, Some(app.palette.active_row_bg));
    }

    #[test]
    fn occurrence_foreground_flattens_composite_git_status_colors() {
        let config: crate::config::Config = toml::from_str(
            r##"[ui.sidebar.spaces]
rows = [[{ token = "git_status", fg = "#123456" }]]
"##,
        )
        .unwrap();
        let spans = resolved_token_spans(
            &[ResolvedToken {
                kind: ResolvedTokenKind::GitStatus {
                    ahead: 2,
                    behind: 1,
                },
                style: config.ui.sidebar.spaces.rows[0][0].parts().1,
            }],
            ("", Style::default()),
            Style::default(),
            Style::default(),
            Style::default(),
            Style::default(),
            &crate::app::state::AppState::test_new().palette,
            20,
        );

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "↑2 ↓1"
        );
        assert!(spans
            .iter()
            .all(|span| { span.style.fg == Some(ratatui::style::Color::Rgb(0x12, 0x34, 0x56)) }));
    }

    #[test]
    fn default_agent_row_gap_packs_rendering_and_scroll_geometry() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.ensure_test_terminals();
        for (workspace, agent) in app.workspaces.iter().zip([Agent::Pi, Agent::Claude]) {
            let pane_id = workspace.tabs[0].root_pane;
            let terminal_id = workspace.tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(agent);
        }
        app.sidebar_agents.rows = vec![vec![crate::config::AgentSidebarToken::Agent]];
        assert_eq!(app.sidebar_agents.row_gap, 0);

        let area = Rect::new(0, 0, 20, 5);
        let metrics = agent_panel_scroll_metrics(&app, area);
        let body = agent_panel_body_rect(area, false);
        let mut terminal = Terminal::new(TestBackend::new(20, 5)).unwrap();
        terminal
            .draw(|frame| render_agent_detail(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();

        assert_eq!(metrics.viewport_rows, 2);
        assert_eq!(metrics.max_offset_from_bottom, 0);
        assert_eq!(row_text(buffer, body.y, body.width), " pi");
        assert_eq!(row_text(buffer, body.y + 1, body.width), " claude");
    }

    #[test]
    fn narrow_agent_rows_preserve_later_tab_tokens() {
        let mut app = crate::app::state::AppState::test_new();
        let mut workspace = Workspace::test_new("very-long-workspace-name");
        let tab_idx = workspace.test_add_tab(Some("logs"));
        let pane_id = workspace.tabs[tab_idx].root_pane;
        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].tabs[tab_idx].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(Agent::Pi);

        let area = Rect::new(0, 0, 18, 20);
        let mut terminal = Terminal::new(TestBackend::new(18, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let (_, agent_area) = expanded_sidebar_sections(area, app.sidebar_section_split);
        let body = agent_panel_body_rect(agent_area, false);
        let first = row_text(buffer, body.y, 17);

        assert!(first.contains("logs"), "rendered row: {first:?}");
        assert!(first.contains('·'), "rendered row: {first:?}");
    }

    #[test]
    fn worktree_new_hit_areas_only_for_repo_headers() {
        let headers = vec![
            crate::app::state::GroupHeaderCardArea {
                name: "herdr".into(),
                collapse_key: "github.com/owner/herdr".into(),
                rect: Rect::new(0, 0, 26, 1),
            },
            crate::app::state::GroupHeaderCardArea {
                name: "mygroup".into(),
                collapse_key: "vg:mygroup".into(),
                rect: Rect::new(0, 1, 26, 1),
            },
        ];
        let hits = worktree_new_hit_areas_from_headers(&headers);
        assert_eq!(hits.len(), 1, "only the repo ProjectHeader gets a +");
        assert_eq!(hits[0].repo_identity, "github.com/owner/herdr");
        assert_eq!(hits[0].rect, Rect::new(23, 0, 3, 1));
    }

    #[test]
    fn render_sidebar_toggle_draws_expanded_collapse_icon() {
        let app = crate::app::state::AppState::test_new();
        let area = Rect::new(0, 0, 26, 20);
        let mut terminal =
            Terminal::new(TestBackend::new(26, 20)).expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_toggle(&app, frame, area, false, &app.palette))
            .expect("sidebar toggle should render");

        let toggle = expanded_sidebar_toggle_rect(area);
        assert_eq!(
            terminal.backend().buffer()[(toggle.x, toggle.y)].symbol(),
            "«"
        );
    }

    #[test]
    fn expanded_sidebar_toggle_sits_inside_sidebar_content() {
        let area = Rect::new(0, 0, 26, 20);
        let toggle = expanded_sidebar_toggle_rect(area);

        assert_eq!(toggle.x, area.x + area.width - 2);
        assert_eq!(toggle.y, area.y + area.height - 1);
    }

    #[test]
    fn all_workspaces_agent_panel_entries_use_workspace_and_optional_tab_labels() {
        let mut app = crate::app::state::AppState::test_new();
        let first = Workspace::test_new("one");
        let first_pane = first.tabs[0].root_pane;
        let mut second = Workspace::test_new("two");
        let second_tab = second.test_add_tab(Some("logs"));
        let second_pane = second.tabs[second_tab].root_pane;

        app.workspaces = vec![first, second];
        app.ensure_test_terminals();
        let first_terminal_id = app.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Pi);
        let second_terminal_id = app.workspaces[1].tabs[second_tab].panes[&second_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&second_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Claude);
        app.active = Some(0);
        app.selected = 0;

        let entries = agent_panel_entries(&app);
        assert_eq!(entries[0].primary_label, "one");
        assert!(entries[0].primary_tab_label.is_none());
        assert_eq!(entries[0].agent_label.as_deref(), Some("pi"));
        assert_eq!(entries[1].primary_label, "two");
        assert_eq!(entries[1].primary_tab_label.as_deref(), Some("logs"));
        assert_eq!(entries[1].agent_label.as_deref(), Some("claude"));
    }

    #[test]
    fn priority_agent_panel_sort_uses_attention_then_space_order() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![
            Workspace::test_new("one"),
            Workspace::test_new("two"),
            Workspace::test_new("three"),
            Workspace::test_new("four"),
        ];
        app.ensure_test_terminals();
        app.active = Some(0);
        app.selected = 0;
        app.agent_panel_sort = crate::app::state::AgentPanelSort::Priority;

        let set_state = |app: &mut crate::app::state::AppState, ws_idx: usize, state| {
            let pane = app.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
                .attached_terminal_id
                .clone();
            let terminal = app.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = state;
        };
        set_state(&mut app, 0, AgentState::Working);
        set_state(&mut app, 1, AgentState::Idle);
        set_state(&mut app, 2, AgentState::Working);
        set_state(&mut app, 3, AgentState::Blocked);

        let done_pane = app.workspaces[1].tabs[0].root_pane;
        app.workspaces[1].tabs[0]
            .panes
            .get_mut(&done_pane)
            .unwrap()
            .seen = false;

        let labels: Vec<String> = agent_panel_entries(&app)
            .into_iter()
            .map(|entry| entry.primary_label)
            .collect();

        assert_eq!(labels, ["four", "two", "one", "three"]);
    }
    #[test]
    fn collapsed_sidebar_uses_all_workspaces_agent_panel_order() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.ensure_test_terminals();
        app.active = Some(0);
        app.selected = 0;
        app.agent_panel_sort = crate::app::state::AgentPanelSort::Priority;
        app.status_indicators = crate::config::StatusIndicatorStyle::Symbols;

        let set_state = |app: &mut crate::app::state::AppState, ws_idx: usize, state| {
            let pane = app.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
                .attached_terminal_id
                .clone();
            let terminal = app.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = state;
        };
        set_state(&mut app, 0, AgentState::Working);
        set_state(&mut app, 1, AgentState::Blocked);

        let area = Rect::new(0, 0, 5, 12);
        let (_, _, detail_area) = collapsed_sidebar_sections(area);
        let first_detail_y = detail_area.y;
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .expect("collapsed sidebar should render");

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(detail_area.x + 2, first_detail_y)].symbol(), "×");
        assert_eq!(
            buffer[(detail_area.x + 2, first_detail_y)].style().fg,
            Some(app.palette.red)
        );
        assert_eq!(buffer[(detail_area.x + 2, detail_area.y + 1)].symbol(), "⠋");
        assert_eq!(
            buffer[(detail_area.x + 2, detail_area.y + 1)].style().fg,
            Some(app.palette.overlay0)
        );
    }

    /// Two agent panes in one workspace plus a second workspace, so the
    /// assertions can tell pane-level highlighting apart from workspace-level.
    fn collapsed_agent_app() -> (crate::app::state::AppState, PaneId, PaneId) {
        let mut app = crate::app::state::AppState::test_new();
        let mut first = Workspace::test_new("one");
        let second_pane = first.test_split(Direction::Horizontal);
        let first_pane = first.tabs[0].root_pane;
        app.workspaces = vec![first, Workspace::test_new("two")];
        app.ensure_test_terminals();

        let terminal_ids: Vec<_> = app
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .flat_map(|tab| tab.panes.values())
            .map(|pane| pane.attached_terminal_id.clone())
            .collect();
        for terminal_id in terminal_ids {
            app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(Agent::Claude);
        }

        (app, first_pane, second_pane)
    }

    fn collapsed_agent_row_styles(
        app: &crate::app::state::AppState,
        area: Rect,
        detail_area: Rect,
        rows: u16,
    ) -> Vec<Vec<ratatui::style::Style>> {
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("test terminal should initialize");
        terminal
            .draw(|frame| render_sidebar_collapsed(app, frame, area))
            .expect("collapsed sidebar should render");
        let buffer = terminal.backend().buffer();
        (0..rows)
            .map(|row| {
                (detail_area.x..detail_area.x + detail_area.width)
                    .map(|x| buffer[(x, detail_area.y + row)].style())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn collapsed_sidebar_highlights_only_the_focused_agent_pane() {
        let (mut app, first_pane, second_pane) = collapsed_agent_app();
        app.active = Some(0);
        app.workspaces[0].tabs[0].layout.focus_pane(second_pane);
        assert!(app.is_active_pane(0, 0, second_pane));
        assert!(!app.is_active_pane(0, 0, first_pane));

        let area = Rect::new(0, 0, 4, 14);
        let (_, _, detail_area) = collapsed_sidebar_sections(area);
        let rows = collapsed_agent_row_styles(&app, area, detail_area, 3);

        let highlighted: Vec<_> = rows
            .iter()
            .filter(|cells| {
                cells
                    .iter()
                    .all(|style| style.bg == Some(app.palette.active_row_bg))
            })
            .collect();
        assert_eq!(
            highlighted.len(),
            1,
            "only the focused agent pane should be highlighted, across the whole row"
        );
        assert_eq!(highlighted[0][0].fg, Some(app.palette.text));

        let muted = rows
            .iter()
            .filter(|cells| cells[0].fg == Some(app.palette.overlay0))
            .count();
        assert_eq!(
            muted, 2,
            "the sibling pane in the active workspace and the other workspace stay muted"
        );
    }

    #[test]
    fn collapsed_sidebar_does_not_highlight_agents_without_active_workspace() {
        let (mut app, _, _) = collapsed_agent_app();
        app.active = None;

        let area = Rect::new(0, 0, 4, 14);
        let (_, _, detail_area) = collapsed_sidebar_sections(area);
        let rows = collapsed_agent_row_styles(&app, area, detail_area, 3);

        for cells in rows {
            assert_eq!(cells[0].fg, Some(app.palette.overlay0));
            for style in cells {
                assert_ne!(style.bg, Some(app.palette.active_row_bg));
            }
        }
    }

    #[test]
    fn collapsed_sidebar_keeps_workspace_status_visible_for_two_digit_positions() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = (1..=10)
            .map(|idx| Workspace::test_new(&format!("workspace-{idx}")))
            .collect();
        app.ensure_test_terminals();

        for ws_idx in 0..app.workspaces.len() {
            let pane = app.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
                .attached_terminal_id
                .clone();
            app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(Agent::Claude);
        }

        let area = Rect::new(0, 0, 4, 25);
        let (workspace_area, _, _) = collapsed_sidebar_sections(area);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .expect("collapsed sidebar should render");

        let tenth_row = workspace_area.y + 9;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(workspace_area.x, workspace_area.y)].symbol(), "1");
        assert_eq!(
            buffer[(workspace_area.x + 1, workspace_area.y)].symbol(),
            " "
        );
        assert_eq!(
            buffer[(workspace_area.x + 2, workspace_area.y)].symbol(),
            "◰"
        );
        assert_eq!(buffer[(workspace_area.x, tenth_row)].symbol(), "1");
        assert_eq!(buffer[(workspace_area.x + 1, tenth_row)].symbol(), "0");
        assert_eq!(buffer[(workspace_area.x + 2, tenth_row)].symbol(), "◰");
    }

    #[test]
    fn collapsed_sidebar_keeps_status_visible_for_two_digit_positions() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = (1..=10)
            .map(|idx| Workspace::test_new(&format!("workspace-{idx}")))
            .collect();
        app.ensure_test_terminals();

        for ws_idx in 0..app.workspaces.len() {
            let pane = app.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
                .attached_terminal_id
                .clone();
            app.terminals.get_mut(&terminal_id).unwrap().detected_agent = Some(Agent::Claude);
        }

        let area = Rect::new(0, 0, 4, 25);
        let (_, _, detail_area) = collapsed_sidebar_sections(area);
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .expect("collapsed sidebar should render");

        let tenth_row = detail_area.y + 9;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(detail_area.x, tenth_row)].symbol(), "1");
        assert_eq!(buffer[(detail_area.x + 1, tenth_row)].symbol(), "0");
        assert_eq!(buffer[(detail_area.x + 2, tenth_row)].symbol(), "○");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn all_workspaces_agent_panel_entries_use_live_root_runtime_cwd_for_workspace_label() {
        let unique = format!(
            "herdr-agent-panel-runtime-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let stale_cwd = root.join("issue-264-nix-support");
        let live_cwd = root.join("herdr");
        std::fs::create_dir_all(stale_cwd.join(".git")).unwrap();
        std::fs::create_dir_all(live_cwd.join(".git")).unwrap();

        let mut app = crate::app::state::AppState::test_new();
        let mut workspace = Workspace::test_new("stale-name");
        workspace.custom_name = None;
        workspace.identity_cwd = stale_cwd.clone();
        let pane = workspace.tabs[0].root_pane;

        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.terminals.get_mut(&terminal_id).unwrap();
        terminal.cwd = stale_cwd;
        terminal.detected_agent = Some(Agent::Pi);
        app.active = Some(0);
        app.selected = 0;

        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            pane,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            &crate::pane::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(crate::render_signal::RenderSignal::new()),
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let mut runtime_registry = TerminalRuntimeRegistry::new();
        runtime_registry.insert(terminal_id, runtime);
        let entries = agent_panel_entries_from(&app, &runtime_registry);
        let primary_label = entries[0].primary_label.clone();

        for (_, runtime) in runtime_registry.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(root);

        assert_eq!(primary_label, "herdr");
    }

    #[test]
    fn all_workspaces_agent_panel_entries_prefer_agent_names_for_agent_identity() {
        let mut app = crate::app::state::AppState::test_new();
        let workspace = Workspace::test_new("bridge");
        let first_pane = workspace.tabs[0].root_pane;

        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let first_terminal_id = app.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Pi);
        app.terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .set_agent_name("planner".into());
        app.active = Some(0);
        app.selected = 0;

        let entries = agent_panel_entries(&app);
        assert_eq!(entries[0].primary_label, "bridge");
        assert_eq!(entries[0].agent_label.as_deref(), Some("planner"));
    }

    #[test]
    fn expanded_sidebar_sections_handle_tiny_heights() {
        let (ws_area, detail_area) = expanded_sidebar_sections(Rect::new(0, 0, 20, 5), 0.9);

        assert_eq!(ws_area, Rect::new(0, 0, 19, 3));
        assert_eq!(detail_area, Rect::new(0, 3, 19, 2));
    }

    #[test]
    fn sidebar_section_divider_is_hidden_for_tiny_heights() {
        let divider = sidebar_section_divider_rect(Rect::new(0, 0, 20, 5), 0.5);

        assert_eq!(divider, Rect::default());
    }

    #[test]
    fn workspace_list_truncates_cjk_branch_without_panic() {
        let mut app = crate::app::state::AppState::test_new();
        let mut ws = Workspace::test_new("repo");
        ws.cached_git_branch = Some("feature/中文-分支-644".into());
        app.workspaces = vec![ws];
        app.active = Some(0);
        app.selected = 0;
        app.mode = Mode::Terminal;
        app.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            ws_idx: 0,
            rect: Rect::new(0, 1, 15, 2),
            indented: false,
        }];

        let mut terminal = Terminal::new(TestBackend::new(15, 6)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 15, 6), false)
            })
            .expect("workspace list should render");
    }

    #[test]
    fn render_branch_bracket_draws_rail_without_member_chevron() {
        // Two checkouts of one repo on the same branch render a branch bracket
        // under a synthesized project header. Only the project header carries a
        // chevron — members never do (regression guard for the synthesized header).
        let mut app = AppState::test_new();
        let identity = "github.com/owner/resume-builder";
        let mut main = git_space_member("main", "key-main", false);
        let mut strider = git_space_member("strider", "key-strider", false);
        for ws in [&mut main, &mut strider] {
            ws.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
            ws.cached_git_branch = Some("main".into());
        }
        app.workspaces = vec![main, strider];
        app.active = Some(0);
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(24, 12)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 24, 12), false)
            })
            .expect("workspace list should render");

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();

        assert!(text.contains("herdr"), "project header label: {text:?}");
        assert!(text.contains("main"), "branch label present: {text:?}");
        assert!(text.contains("strider"), "member name present: {text:?}");
        assert_eq!(
            text.matches('▾').count(),
            0,
            "ProjectHeader has no chevron; GroupHeader (visual groups) does: {text:?}"
        );
    }

    #[test]
    fn render_branch_bracket_shows_pr_badge() {
        // A workspace whose branch has an open PR shows a `#<number>` badge on
        // the branch bracket header.
        let mut app = AppState::test_new();
        let mut ws = git_space_member("main", "key-pr", false);
        ws.cached_git_branch = Some("feature".into());
        ws.cached_check_status = Some(crate::workspace::WorkspaceCheckStatus {
            pr: Some(crate::workspace::PrSummary {
                number: 42,
                title: "feat: thing".into(),
                state: "OPEN".into(),
                url: "https://example.com/pr/42".into(),
                mergeable: None,
            }),
            checks: vec![],
            error: None,
        });
        app.workspaces = vec![ws];
        app.active = Some(0);
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(24, 12)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 24, 12), false)
            })
            .expect("workspace list should render");

        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();

        assert!(text.contains("#42"), "PR badge present: {text:?}");
    }

    fn workspace_with_worktree_space(
        name: &str,
        key: Option<&str>,
        checkout_key: &str,
    ) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::test_new(name);
        ws.cached_git_branch = None;
        if let Some(key) = key {
            let is_linked = name != "main";
            ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
                key: key.into(),
                label: "herdr".into(),
                repo_root: std::path::PathBuf::from("/repo/herdr"),
                checkout_path: std::path::PathBuf::from(checkout_key),
                is_linked_worktree: is_linked,
            });
            ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
                key: key.into(),
                repo_identity: key.into(),
                checkout_key: checkout_key.into(),
                repo_name: "herdr".into(),
                repo_root: std::path::PathBuf::from("/repo/herdr"),
                is_linked_worktree: is_linked,
            });
        }
        ws
    }

    fn workspace_with_git_space(name: &str, key: &str) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::test_new(name);
        ws.cached_git_branch = None;
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: key.into(),
            repo_identity: key.into(),
            checkout_key: format!("/repo/{name}"),
            repo_name: "herdr".into(),
            repo_root: std::path::PathBuf::from(format!("/repo/{name}")),
            is_linked_worktree: false,
        });
        ws
    }

    fn git_space_member(
        name: &str,
        key: &str,
        is_linked_worktree: bool,
    ) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::test_new(name);
        ws.cached_git_branch = None;
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: key.into(),
            repo_identity: key.into(),
            checkout_key: format!("/repo/{name}"),
            repo_name: "herdr".into(),
            repo_root: std::path::PathBuf::from("/repo/herdr"),
            is_linked_worktree,
        });
        ws
    }

    #[test]
    fn parent_workspace_row_stays_clickable_when_grouped() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
        ];
        app.sidebar_spaces.row_gap = 1;

        let (cards, headers) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 40));

        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "herdr");
        assert_eq!(cards[0].ws_idx, 0);
        assert!(cards[0].indented);
        assert_eq!(cards[1].ws_idx, 1);
        assert!(cards[1].indented);
        assert_eq!(cards[1].rect.y, cards[0].rect.y + cards[0].rect.height);
    }

    #[test]
    fn linked_only_worktree_members_get_synthetic_repo_header() {
        // Option C: with no main checkout open, linked worktrees of the same repo
        // group under a synthesized repo header instead of rendering flat.
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
            workspace_with_worktree_space("review", Some("repo-key"), "/repo/herdr-review"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn compact_space_group_scroll_offset_can_start_inside_group() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("one", Some("repo-key"), "/repo/herdr-one"),
            workspace_with_worktree_space("two", Some("repo-key"), "/repo/herdr-two"),
        ];
        let area = Rect::new(0, 0, 30, 20);
        app.workspace_scroll = normalized_workspace_scroll(&app, area, 2);

        let (cards, headers) = compute_workspace_list_areas(&app, area);

        assert!(headers.is_empty());
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].ws_idx, 1);
        assert_eq!(cards[1].ws_idx, 2);
    }

    #[test]
    fn workspace_scroll_metrics_count_display_entries_not_raw_workspaces() {
        let mut app = AppState::test_new();
        // `Workspace::test_new` derives `cached_git_branch` from the real
        // process cwd (workspace.rs:1333) — a named-branch checkout picks one
        // up, a detached-HEAD CI checkout does not, which silently changes
        // this workspace from branchless to branched and shifts the entry
        // count. Reset it like the sibling test below does, so the fixture
        // stays hermetic instead of depending on how the test runner's
        // working tree happens to be checked out.
        let mut notes = Workspace::test_new("notes");
        notes.cached_git_branch = None;
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
            notes,
        ];
        app.collapsed_space_keys.insert("repo-key".into());
        app.active = None;
        app.mode = Mode::Terminal;

        // +1 row vs. the original fixture height to absorb the always-on
        // "+ run command…" Programs launcher row reserved at the bottom of
        // the workspace list, so this still exercises the "everything fits"
        // (zero scroll) case the test name describes.
        let ws_area = Rect::new(0, 0, 30, 7);
        let metrics = workspace_list_scroll_metrics(&app, ws_area);

        // 2 display entries (the collapsed repo-key header + branchless
        // "notes"), not 3 raw workspaces — the case this test's name names.
        assert_eq!(metrics.viewport_rows, 2);
        assert_eq!(metrics.max_offset_from_bottom, 0);
        assert_eq!(metrics.offset_from_bottom, 0);
    }

    #[test]
    fn workspace_scroll_offset_applies_to_group_children() {
        let mut app = AppState::test_new();
        let mut notes = Workspace::test_new("notes");
        notes.cached_git_branch = None;
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
            notes,
        ];
        app.collapsed_space_keys.insert("repo-key".into());
        app.active = None;
        app.mode = Mode::Terminal;
        app.workspace_scroll = 1;

        let (cards, headers) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 12));

        assert!(headers.is_empty());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].ws_idx, 2);
    }

    #[test]
    fn workspace_list_entries_group_multiple_workspaces_in_same_git_space() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_group_non_contiguous_explicit_members() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_git_space("normal", "other-key"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Close,
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "other-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_group_same_repo_git_workspaces() {
        // Plain checkouts sharing a repo (git_space) group under a synthesized
        // project header, branch-subgrouped beneath it.
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_git_space("one", "repo-key"),
            workspace_with_git_space("two", "repo-key"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_group_clones_by_repo_identity() {
        // Separate clones of one GitHub repo have distinct worktree keys but a
        // shared repo_identity; they must still collapse under one project group.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/resume-builder";
        let mut main = git_space_member("main", "key-main", false);
        let mut strider = git_space_member("strider", "key-strider", false);
        let mut zep = git_space_member("apply-zep", "key-zep", false);
        for ws in [&mut main, &mut strider, &mut zep] {
            ws.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        }
        app.workspaces = vec![main, strider, zep];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "github.com/owner/resume-builder".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_distinct_repo_identities_stay_flat() {
        // Same worktree key would have grouped before, but distinct repo_identities
        // must not group: identity is now the grouping authority.
        let mut app = AppState::test_new();
        let mut a = git_space_member("a", "shared-key", false);
        let mut b = git_space_member("b", "shared-key", false);
        a.cached_git_space.as_mut().unwrap().repo_identity = "github.com/owner/a".into();
        b.cached_git_space.as_mut().unwrap().repo_identity = "github.com/owner/b".into();
        app.workspaces = vec![a, b];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "github.com/owner/a".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                    rail: BranchRail::Close,
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "github.com/owner/b".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_attach_same_repo_git_workspace_to_group() {
        // Option A: a plain same-repo checkout attaches to the repo group as a child,
        // even without explicit Herdr worktree membership.
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_git_space("scratch", "repo-key"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_group_non_adjacent_repo_checkouts_without_membership() {
        // A non-linked main checkout plus linked worktrees of the same repo, with an
        // unrelated workspace interleaved, all nest under a synthesized project header.
        let mut app = AppState::test_new();
        app.workspaces = vec![
            git_space_member("herdr", "repo-key", false),
            git_space_member("unrelated", "other-key", false),
            git_space_member("right-sidebar", "repo-key", true),
            git_space_member("ajusta", "repo-key", true),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 3,
                    indented: true,
                    rail: BranchRail::Close,
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "other-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_synthesize_header_when_no_main_checkout_open() {
        // Option C: only linked worktrees of a repo are open (no main checkout).
        // A synthetic repo header is emitted with all worktrees nested under it.
        let mut app = AppState::test_new();
        app.workspaces = vec![
            git_space_member("right-sidebar", "repo-key", true),
            git_space_member("ajusta", "repo-key", true),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_collapsed_synthetic_repo_header_hides_children() {
        // Collapsing a synthetic repo header leaves only the header row.
        let mut app = AppState::test_new();
        app.workspaces = vec![
            git_space_member("right-sidebar", "repo-key", true),
            git_space_member("ajusta", "repo-key", true),
        ];
        app.collapsed_space_keys.insert("repo-key".into());
        app.active = None;
        app.mode = Mode::Terminal;

        assert_eq!(
            workspace_list_entries(&app),
            vec![WorkspaceListEntry::ProjectHeader {
                name: "herdr".into(),
                collapse_key: "repo-key".into(),
                indented: false,
                branch: None,
            }]
        );
    }

    #[test]
    fn workspace_list_entries_leave_single_git_and_non_git_workspaces_flat() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_git_space("one", "repo-key"),
            workspace_with_worktree_space("notes", None, "/notes"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                    rail: BranchRail::Close,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn collapsed_group_hides_inactive_children_but_keeps_active_visible() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
        ];
        app.active = Some(1);
        app.mode = Mode::Terminal;
        app.collapsed_space_keys.insert("repo-key".into());

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::None,
                },
            ]
        );

        app.active = None;
        app.mode = Mode::Terminal;
        assert_eq!(
            workspace_list_entries(&app),
            vec![WorkspaceListEntry::ProjectHeader {
                name: "herdr".into(),
                collapse_key: "repo-key".into(),
                indented: false,
                branch: None,
            }]
        );
    }

    #[test]
    fn collapsed_group_keeps_selected_child_visible_in_navigate_mode() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
        ];
        app.mode = Mode::Navigate;
        app.selected = 1;
        app.active = Some(1);
        app.collapsed_space_keys.insert("repo-key".into());

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    // --- Visual group tests ---

    #[test]
    fn single_member_visual_group_renders_header_and_indented_child() {
        let mut app = AppState::test_new();
        let mut ws = Workspace::test_new("alpha");
        ws.visual_group = Some("g1".into());
        ws.cached_git_branch = None;
        app.workspaces = vec![ws];

        let entries = workspace_list_entries(&app);

        // Visual-group member has no git_space and no branch: emitted as
        // indented Workspace with rail None (no BranchHeader).
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::GroupHeader {
                    name: "g1".into(),
                    collapse_key: "vg:g1".into()
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn multi_member_visual_group_all_under_header() {
        let mut app = AppState::test_new();
        let mut ws0 = Workspace::test_new("alpha");
        ws0.visual_group = Some("g1".into());
        ws0.cached_git_branch = None;
        let mut ws1 = Workspace::test_new("beta");
        ws1.visual_group = Some("g1".into());
        ws1.cached_git_branch = None;
        app.workspaces = vec![ws0, ws1];

        let entries = workspace_list_entries(&app);

        // Both members have no branch: emitted as indented Workspaces with
        // rail None, nested beneath the GroupHeader.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::GroupHeader {
                    name: "g1".into(),
                    collapse_key: "vg:g1".into()
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn hash_prefixed_workspaces_auto_group_as_channels() {
        // orc channels are created as `--label "#<name>"` and hosted by whatever
        // checkout was convenient, so repo grouping scatters them. They collect
        // under one channel group instead, with unrelated workspaces untouched.
        let mut app = AppState::test_new();
        let mut chan_a = Workspace::test_new("canal-ary");
        chan_a.set_custom_name("#canal-ary".into());
        chan_a.cached_git_branch = None;
        let mut plain = Workspace::test_new("orchestrator");
        plain.cached_git_branch = None;
        let mut chan_b = Workspace::test_new("part3");
        chan_b.set_custom_name("#part3-model-status".into());
        chan_b.cached_git_branch = None;
        app.workspaces = vec![chan_a, plain, chan_b];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::GroupHeader {
                    // Literal, not CHANNEL_GROUP_NAME: asserting against the
                    // constant the production code reads means a rename changes
                    // both sides together and the test cannot see it.
                    name: "channels".into(),
                    collapse_key: "vg:channels".into(),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Close,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn explicit_group_beats_the_channel_prefix() {
        // The `#` rule is a default, not a cage: a channel filed into a group by
        // hand stays there.
        let mut app = AppState::test_new();
        let mut chan = Workspace::test_new("canal-ary");
        chan.set_custom_name("#canal-ary".into());
        chan.visual_group = Some("mine".into());
        chan.cached_git_branch = None;
        app.workspaces = vec![chan];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries.first(),
            Some(&WorkspaceListEntry::GroupHeader {
                name: "mine".into(),
                collapse_key: "vg:mine".into(),
            })
        );
    }

    #[test]
    fn workspace_row_renders_configured_custom_token() {
        // `bora workspace report-metadata` is how an outside process (the orc
        // channel code) flags unread traffic. This exercises the real
        // `[ui.sidebar.spaces] rows` config path (`tokens::space_rows` +
        // `resolved_token_spans`), not an ad-hoc suffix.
        let mut app = AppState::test_new();
        app.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Custom(
            "unread".into(),
        )]];
        let mut ws = Workspace::test_new("canal-ary");
        ws.set_custom_name("#canal-ary".into());
        ws.cached_git_branch = None;
        ws.metadata_tokens.patch(
            std::collections::HashMap::from([("unread".to_string(), Some("3 msg".to_string()))]),
            None,
            std::time::Instant::now(),
        );
        app.workspaces = vec![ws];

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 40, 8);
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");
        let rows: Vec<String> = (0..8)
            .map(|row| {
                (0..40)
                    .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect();

        assert!(
            rows.iter().any(|row| row.contains("3 msg")),
            "configured $unread token is drawn on the workspace row via the real config path: {rows:?}"
        );
    }

    #[test]
    fn agent_row_renders_configured_custom_token() {
        // Same restoration, agent side: a configured `$pr` custom token in
        // `[ui.sidebar.agents] rows` must draw on the agent panel's status
        // line via `resolved_agent_rows` + `resolved_token_spans`.
        let mut app = AppState::test_new();
        app.sidebar_agents.rows = vec![vec![crate::config::AgentSidebarToken::Custom("pr".into())]];
        let ws = Workspace::test_new("agent-ws");
        let root_pane = ws.tabs[0].root_pane;
        let terminal_id = ws.terminal_id(root_pane).unwrap().clone();
        let mut terminal_state =
            crate::terminal::TerminalState::new(terminal_id.clone(), "/tmp".into());
        terminal_state.set_agent_name("planner".into());
        terminal_state.metadata_tokens.patch(
            std::collections::HashMap::from([("pr".to_string(), Some("#42".to_string()))]),
            None,
            std::time::Instant::now(),
        );
        app.workspaces = vec![ws];
        app.terminals.insert(terminal_id, terminal_state);

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 40, 8);
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("test terminal");
        terminal
            .draw(|frame| render_agent_detail(&app, &runtimes, frame, area))
            .expect("agent detail should render");
        let rows: Vec<String> = (0..8)
            .map(|row| {
                (0..40)
                    .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect();

        assert!(
            rows.iter().any(|row| row.contains("#42")),
            "configured $pr token is drawn on the agent panel row: {rows:?}"
        );
    }

    #[test]
    fn channels_leave_their_host_repo_group_in_the_live_fleet_shape() {
        // Transcribed from the running session, because smaller branchless cases
        // could not reproduce the failure: every `#` channel is a NON-linked
        // checkout of a repo that also has real work workspaces, since orc hosts
        // all channels in the orchestrator hub. That combination made the channel
        // group swallow the entire repo — project header, branches and all — while
        // leaving the channels themselves inside the repo's bracket.
        let mut app = AppState::test_new();
        let mk = |name: &str, repo: &str, linked: bool, branch: &str, custom: Option<&str>| {
            let mut ws = git_space_member_on_branch(name, repo, linked, branch);
            ws.cached_git_space.as_mut().unwrap().repo_identity = repo.into();
            ws.cached_git_space.as_mut().unwrap().repo_name = repo.into();
            if let Some(custom) = custom {
                ws.set_custom_name(custom.into());
            }
            ws
        };
        app.workspaces = vec![
            mk("orchestrator", "orchestrator", false, "main", None),
            mk("bora", "bora", false, "main", None),
            mk("canal", "orchestrator", false, "main", Some("#canal-ary")),
            mk("orcbin", "orchestrator", true, "orcbin", None),
            mk("orchestrator-review", "orchestrator", false, "main", None),
            mk(
                "part3",
                "orchestrator",
                false,
                "main",
                Some("#part3-model-status"),
            ),
        ];

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 34, 20);
        let mut terminal = Terminal::new(TestBackend::new(34, 20)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("render");
        let rows: Vec<String> = (0..20)
            .map(|row| {
                (0..34)
                    .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();

        let repo_at = rows
            .iter()
            .position(|row| row.starts_with("╭─orchestrator"))
            .unwrap_or_else(|| panic!("orchestrator keeps a top-level project header: {rows:?}"));
        let group_at = rows
            .iter()
            .position(|row| row.contains("channels"))
            .unwrap_or_else(|| panic!("channels get their own group: {rows:?}"));
        assert!(
            group_at < repo_at,
            "channels form their own leading block, above the repo groups: {rows:?}"
        );
        // No `#` row may sit inside the repo bracket, which now runs from the repo
        // header to the end of the drawn rows.
        let inside_repo = &rows[repo_at..];
        assert!(
            !inside_repo.iter().any(|row| row.contains('#')),
            "no channel is left inside the repo bracket: {inside_repo:?}"
        );
        assert!(
            repo_at > 0 && !rows[..repo_at].is_empty(),
            "the repo block still renders after the channel block: {rows:?}"
        );

        // The channel group holds both channels, and its rail closes on the last.
        let channel_rows = &rows[group_at + 1..group_at + 3];
        assert!(
            channel_rows[0].starts_with('│') && channel_rows[0].contains("#canal-ary"),
            "first channel rides the spine: {channel_rows:?}"
        );
        assert!(
            channel_rows[1].starts_with("╰── ") && channel_rows[1].contains("#part3-model-status"),
            "last channel closes the rail at the bottom of the group: {channel_rows:?}"
        );
    }

    #[test]
    fn channel_group_name_is_configurable_without_changing_membership() {
        // The word is user-facing, so it is config. What counts as a channel keys
        // off the `#` label, so renaming the group must move nobody in or out.
        let mut app = AppState::test_new();
        app.channel_group_name = "canais".to_string();
        let mut chan = Workspace::test_new("canal");
        chan.set_custom_name("#canal-ary".into());
        chan.cached_git_branch = None;
        let mut plain = Workspace::test_new("orchestrator");
        plain.cached_git_branch = None;
        app.workspaces = vec![chan, plain];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries.first(),
            Some(&WorkspaceListEntry::GroupHeader {
                name: "canais".into(),
                collapse_key: "vg:canais".into(),
            }),
            "the configured name is what the group header carries: {entries:?}"
        );
        let grouped = entries
            .iter()
            .filter(|entry| matches!(entry, WorkspaceListEntry::Workspace { indented: true, .. }))
            .count();
        assert_eq!(
            grouped, 1,
            "renaming the group moves nobody in or out: {entries:?}"
        );
    }

    #[test]
    fn collapsed_visual_group_shows_only_header() {
        let mut app = AppState::test_new();
        let mut ws0 = Workspace::test_new("alpha");
        ws0.visual_group = Some("g1".into());
        let mut ws1 = Workspace::test_new("beta");
        ws1.visual_group = Some("g1".into());
        app.workspaces = vec![ws0, ws1];
        app.collapsed_space_keys.insert("vg:g1".into());

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![WorkspaceListEntry::GroupHeader {
                name: "g1".into(),
                collapse_key: "vg:g1".into()
            },]
        );
    }

    #[test]
    fn visual_group_wraps_worktree_group() {
        let mut app = AppState::test_new();
        // ws0 is a worktree parent AND has a visual_group — vg wraps the worktree group.
        let mut ws0 = workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr");
        ws0.visual_group = Some("g1".into());
        let ws1 = workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue");
        app.workspaces = vec![ws0, ws1];

        let entries = workspace_list_entries(&app);

        // Visual group header, then a synthesized project header, then the repo's
        // checkouts nested under it.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::GroupHeader {
                    name: "g1".into(),
                    collapse_key: "vg:g1".into()
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: true,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn branchless_members_stay_inside_bracket() {
        // Regression (juno_brain): one checkout with a detected branch plus
        // members whose branch is unknown. Branchless members must render
        // inside the bracket (spine rails) and the LAST row closes it —
        // never rail-less rows dangling after the bracket closed.
        let mut app = AppState::test_new();
        let mut ws0 = workspace_with_worktree_space("main", Some("repo-key"), "/repo/juno");
        ws0.cached_git_branch = Some("init".into());
        let ws1 =
            workspace_with_worktree_space("dashboard-v0", Some("repo-key"), "/repo/juno-dash");
        let ws2 = workspace_with_worktree_space("juno-2", Some("repo-key"), "/repo/juno-2");
        app.workspaces = vec![ws0, ws1, ws2];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "init".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn all_branchless_bracket_group_still_closes() {
        // No branch detected anywhere: header has no [branch] label, members
        // still get spine rails and the last one closes the bracket.
        let mut app = AppState::test_new();
        let ws0 = workspace_with_worktree_space("main", Some("repo-key"), "/repo/juno");
        let ws1 = workspace_with_worktree_space("child", Some("repo-key"), "/repo/juno-child");
        app.workspaces = vec![ws0, ws1];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn ungrouped_workspaces_render_flat() {
        let mut app = AppState::test_new();
        let mut ws0 = Workspace::test_new("alpha");
        ws0.cached_git_branch = None;
        let mut ws1 = Workspace::test_new("beta");
        ws1.cached_git_branch = None;
        app.workspaces = vec![ws0, ws1];

        let entries = workspace_list_entries(&app);

        // Non-git workspaces with no branch render flat with no header.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn toggle_false_emits_flat_entries_no_headers() {
        let mut app = AppState::test_new();
        app.group_workspaces_by_repo = false;
        let ws0 = git_space_member("main", "repo-key", false);
        let ws1 = git_space_member("child", "repo-key", true);
        app.workspaces = vec![ws0, ws1];

        let entries = workspace_list_entries(&app);

        // Flat mode: exactly one Workspace entry per workspace, in vec
        // order, no ProjectHeader/BranchHeader/GroupHeader synthesis at all
        // — even though these two share a repo and would bracket-group.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn toggle_true_groups_same_repo() {
        // Control: with the toggle at its default (true), the same pair
        // still brackets into a repo header the way it always has.
        let mut app = AppState::test_new();
        assert!(app.group_workspaces_by_repo);
        let ws0 = git_space_member("main", "repo-key", false);
        let ws1 = git_space_member("child", "repo-key", true);
        app.workspaces = vec![ws0, ws1];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "repo-key".into(),
                    indented: false,
                    branch: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn retoggle_restores_grouping() {
        // Grouping is recomputed from workspace data on every call, never
        // cached: flip off, mutate the vec order the way a flat-mode drag
        // would, flip back on, and the bracket must reflect the new order.
        let mut app = AppState::test_new();
        let ws0 = git_space_member("main", "repo-key", false);
        let ws1 = git_space_member("child", "repo-key", true);
        app.workspaces = vec![ws0, ws1];

        app.group_workspaces_by_repo = false;
        let flat = workspace_list_entries(&app);
        assert!(flat
            .iter()
            .all(|e| matches!(e, WorkspaceListEntry::Workspace { .. })));

        // Simulate the drag reorder a user performs while flat.
        app.workspaces.swap(0, 1);

        app.group_workspaces_by_repo = true;
        let grouped = workspace_list_entries(&app);
        assert!(matches!(
            grouped[0],
            WorkspaceListEntry::ProjectHeader { .. }
        ));
        assert_eq!(grouped.len(), 3);
        let WorkspaceListEntry::Workspace {
            ws_idx: first_idx, ..
        } = grouped[1]
        else {
            panic!("expected a Workspace entry");
        };
        let WorkspaceListEntry::Workspace {
            ws_idx: second_idx, ..
        } = grouped[2]
        else {
            panic!("expected a Workspace entry");
        };
        // Post-swap, "child" sits at vec index 0 and "main" at index 1;
        // emission order follows the vec, so "child" now comes first.
        assert_eq!(
            app.workspaces[first_idx].custom_name.as_deref(),
            Some("child")
        );
        assert_eq!(
            app.workspaces[second_idx].custom_name.as_deref(),
            Some("main")
        );
    }

    #[test]
    fn group_header_areas_allocated_for_visual_groups() {
        let mut app = AppState::test_new();
        let mut ws0 = Workspace::test_new("alpha");
        ws0.visual_group = Some("mygroup".into());
        ws0.cached_git_branch = None;
        let mut ws1 = Workspace::test_new("beta");
        ws1.visual_group = Some("mygroup".into());
        ws1.cached_git_branch = None;
        app.workspaces = vec![ws0, ws1];

        let (cards, headers) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 40));

        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "mygroup");
        assert_eq!(cards.len(), 2);
        // Members inside a visual group are indented.
        assert!(cards[0].indented);
        assert!(cards[1].indented);
        // Group header must appear before its member cards.
        assert!(headers[0].rect.y < cards[0].rect.y);
    }

    // --- Branch sub-grouping tests ---

    /// Helper: create a git_space_member with a specific branch.
    fn git_space_member_on_branch(
        name: &str,
        key: &str,
        is_linked: bool,
        branch: &str,
    ) -> crate::workspace::Workspace {
        let mut ws = git_space_member(name, key, is_linked);
        ws.cached_git_branch = Some(branch.into());
        ws
    }

    #[test]
    fn clones_on_same_branch_get_one_bracket_with_rail() {
        let mut app = AppState::test_new();
        let identity = "github.com/owner/resume-builder";
        let mut main_ws = git_space_member("main", "key-main", false);
        main_ws.cached_git_branch = Some("main".into());
        main_ws.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut strider = git_space_member("strider", "key-strider", false);
        strider.cached_git_branch = Some("main".into());
        strider.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        app.workspaces = vec![main_ws, strider];

        let entries = workspace_list_entries(&app);

        // Both checkouts are on branch "main"; the branch folds into the
        // project header and its last member closes the bracket.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "github.com/owner/resume-builder".into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "main".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn single_ws_branch_emits_bracket() {
        // A single git workspace with a branch folds that branch into the
        // project header; the no-branch parent stays inside the bracket and
        // its last row closes it (Close).
        let mut app = AppState::test_new();
        let identity = "github.com/owner/site";
        let mut parent = git_space_member("site", "key-parent", false);
        parent.cached_git_branch = None;
        parent.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut child = git_space_member("main", "key-child", false);
        child.cached_git_branch = Some("main".into());
        child.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        app.workspaces = vec![parent, child];

        let entries = workspace_list_entries(&app);

        // ProjectHeader{branch: main} + Workspace{Spine} for the branched child
        // + Workspace{Close} for the no-branch parent inside the bracket.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: identity.into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "main".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn workspace_card_area_rect_is_single_row() {
        let mut app = AppState::test_new();
        app.workspaces = vec![Workspace::test_new("alpha")];

        let (cards, _) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 20));

        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].rect.height, 1,
            "card rect is a single row (name + inline dots)"
        );
    }

    #[test]
    fn multiple_branches_in_one_project_emit_multiple_brackets() {
        // Three branches + a no-branch parent: the first branch folds into the
        // project header, the no-branch parent stays inside the bracket right
        // after the folded members. `feat/b` and `feat/c` each hold a single
        // auto-named workspace (no `custom_name`), so per the collapse rule
        // their headers fold that lone workspace INTO themselves instead of
        // printing the branch name twice — no separate `Workspace` row for
        // either. `feat/c` is also the group's last row, so its collapsed
        // header draws the closing elbow itself.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut parent = git_space_member("proj", "key-p", false);
        parent.cached_git_branch = None;
        parent.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_b = git_space_member_on_branch("feature-b", "key-b", false, "feat/b");
        ws_b.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_b.custom_name = None;
        let mut ws_c = git_space_member_on_branch("feature-c", "key-c", false, "feat/c");
        ws_c.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_c.custom_name = None;
        app.workspaces = vec![parent, ws_a, ws_b, ws_c];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: identity.into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "feat/a".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::BranchHeader {
                    label: "feat/b".into(),
                    ahead: 0,
                    behind: 0,
                    indented: false,
                    last: false,
                    ws_idx: Some(2),
                },
                WorkspaceListEntry::BranchHeader {
                    label: "feat/c".into(),
                    ahead: 0,
                    behind: 0,
                    indented: false,
                    last: true,
                    ws_idx: Some(3),
                },
            ]
        );

        // Render pass: the bracket prefixes land at column 0.
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 40, 12);
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");
        let row_text = |row: u16| -> String {
            (0..40)
                .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                .collect()
        };
        let body_y = WORKSPACE_SECTION_HEADER_ROWS;
        assert!(
            row_text(body_y).starts_with("╭─") && row_text(body_y).contains("feat/a"),
            "header opens bracket with folded branch: {:?}",
            row_text(body_y)
        );
        assert!(
            row_text(body_y + 1).starts_with('│'),
            "folded member on spine: {:?}",
            row_text(body_y + 1)
        );
        assert!(
            row_text(body_y + 2).starts_with('│'),
            "loose no-branch member stays on the spine: {:?}",
            row_text(body_y + 2)
        );
        assert!(
            row_text(body_y + 3).starts_with("├── ") && row_text(body_y + 3).contains("feat/b"),
            "collapsed feat/b header is a tee, another branch still follows: {:?}",
            row_text(body_y + 3)
        );
        assert!(
            row_text(body_y + 4).starts_with("╰── ") && row_text(body_y + 4).contains("feat/c"),
            "collapsed feat/c header IS the group's last row, so it closes the bracket itself: {:?}",
            row_text(body_y + 4)
        );
    }

    #[test]
    fn single_auto_named_worktree_on_a_branch_collapses_to_one_row() {
        // feat/b's only member has no custom_name (still named after its
        // checkout): the header and the would-be child row would show the
        // identical string, so the header folds that workspace into itself.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_b = git_space_member_on_branch("feature-b", "key-b", false, "feat/b");
        ws_b.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_b.custom_name = None;
        app.workspaces = vec![ws_a, ws_b];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: identity.into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "feat/a".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::BranchHeader {
                    label: "feat/b".into(),
                    ahead: 0,
                    behind: 0,
                    indented: false,
                    last: true,
                    ws_idx: Some(1),
                },
            ],
            "no separate child row is emitted for the collapsed branch"
        );
    }

    #[test]
    fn two_workspaces_on_the_same_branch_keep_header_and_both_rows() {
        // A worktree can host two workspaces: even with no custom_name, 2+
        // members must never collapse — the owner's explicit guard.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_b1 = git_space_member_on_branch("feature-b", "key-b1", true, "feat/b");
        ws_b1.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_b1.custom_name = None;
        let mut ws_b2 = git_space_member_on_branch("feature-b-2", "key-b2", true, "feat/b");
        ws_b2.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_b2.custom_name = None;
        app.workspaces = vec![ws_a, ws_b1, ws_b2];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: identity.into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "feat/a".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::BranchHeader {
                    label: "feat/b".into(),
                    ahead: 0,
                    behind: 0,
                    indented: false,
                    last: false,
                    ws_idx: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn single_worktree_with_custom_name_does_not_collapse() {
        // A workspace the user renamed by hand keeps its own visible row
        // even though it is still the branch's only member.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_b = git_space_member_on_branch("feature-b", "key-b", false, "feat/b");
        ws_b.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_b.custom_name = Some("my-renamed-space".into());
        app.workspaces = vec![ws_a, ws_b];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: identity.into(),
                    indented: false,
                    branch: Some(ProjectHeaderBranch {
                        label: "feat/a".into(),
                        ahead: 0,
                        behind: 0,
                    }),
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::BranchHeader {
                    label: "feat/b".into(),
                    ahead: 0,
                    behind: 0,
                    indented: false,
                    last: false,
                    ws_idx: None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Close,
                },
            ]
        );
    }

    #[test]
    fn single_worktree_renamed_to_its_own_branch_still_collapses() {
        // Measured live: a worktree workspace can carry a custom name identical
        // to its branch. The repetition is what the reader sees, so the test is
        // the repeated string, not whether a human typed it.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut named = git_space_member_on_branch("token", "key-b", false, "orc-channel-token");
        named.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        named.custom_name = Some("orc-channel-token".into());
        let mut short = git_space_member_on_branch("badge", "key-c", false, "ary/orc-canal-badge");
        short.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        short.custom_name = Some("orc-canal-badge".into());
        app.workspaces = vec![ws_a, named, short];

        let entries = workspace_list_entries(&app);

        let collapsed: Vec<Option<usize>> = entries
            .iter()
            .filter_map(|entry| match entry {
                WorkspaceListEntry::BranchHeader { ws_idx, .. } => Some(*ws_idx),
                _ => None,
            })
            .collect();
        assert_eq!(
            collapsed,
            vec![Some(1), Some(2)],
            "a name identical to the branch collapses, and so does one matching \
             the branch's last segment: {entries:?}"
        );
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, WorkspaceListEntry::Workspace { ws_idx: 1 | 2, .. })),
            "neither collapsed workspace keeps a duplicate child row: {entries:?}"
        );
    }

    #[test]
    fn collapsed_branch_header_row_is_a_clickable_workspace_card() {
        // The old child row was clickable; a collapsed row that cannot be
        // clicked is a regression. This is the load-bearing assertion for
        // `ws_idx`.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_b = git_space_member_on_branch("feature-b", "key-b", false, "feat/b");
        ws_b.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        ws_b.custom_name = None;
        app.workspaces = vec![ws_a, ws_b];

        let (cards, _headers) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 20));

        assert!(
            cards.iter().any(|c| c.ws_idx == 1),
            "collapsed branch header must register a clickable card for its workspace: {cards:?}"
        );
    }

    #[test]
    fn tab_dot_states_returns_per_tab_aggregate() {
        let mut app = AppState::test_new();
        let mut ws = Workspace::test_new("multi");
        let _tab1 = ws.test_add_tab(Some("second"));
        app.workspaces = vec![ws];
        app.ensure_test_terminals();

        let dots = tab_dot_states(&app.workspaces[0], &app.terminals);

        assert_eq!(dots.len(), 2, "should have one dot per tab");
        // Default state for unknown terminals.
        for (state, _seen) in &dots {
            assert!(
                matches!(state, AgentState::Unknown | AgentState::Idle),
                "default tab dot state should be Unknown or Idle"
            );
        }
    }

    #[test]
    fn entry_row_height_group_header_is_one() {
        let entries = vec![WorkspaceListEntry::GroupHeader {
            name: "g".into(),
            collapse_key: "k".into(),
        }];
        assert_eq!(entry_row_height(&entries[0], &entries, 0), 1);
    }

    #[test]
    fn entry_row_height_branch_header_is_one() {
        let entries = vec![WorkspaceListEntry::BranchHeader {
            label: "main".into(),
            ahead: 0,
            behind: 0,
            indented: false,
            last: false,
            ws_idx: None,
        }];
        assert_eq!(entry_row_height(&entries[0], &entries, 0), 1);
    }

    #[test]
    fn entry_row_height_workspace_is_one_row() {
        let entries = vec![
            WorkspaceListEntry::Workspace {
                ws_idx: 0,
                indented: true,
                rail: BranchRail::Spine,
            },
            WorkspaceListEntry::Workspace {
                ws_idx: 1,
                indented: true,
                rail: BranchRail::None,
            },
        ];
        // Every workspace is a single row: name + inline dots.
        assert_eq!(entry_row_height(&entries[0], &entries, 0), 1);
        assert_eq!(entry_row_height(&entries[1], &entries, 1), 1);
    }

    // Characterization: pins the lockstep entries system for a git repo group
    // (synthesized project header + branch bracket + members + footer)
    // followed by a flat workspace. All three lockstep passes (visible-count,
    // geometry, render) must agree with `entry_row_height` applied to the
    // same `workspace_list_entries` sequence.
    #[test]
    fn workspace_list_lockstep_passes_agree_for_git_repo_group() {
        let mut app = AppState::test_new();
        let identity = "github.com/owner/herdr";
        let mut main = git_space_member("main", "key-main", false);
        let mut issue = git_space_member("issue", "key-issue", true);
        for ws in [&mut main, &mut issue] {
            ws.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
            ws.cached_git_branch = Some("main".into());
        }
        let mut notes = Workspace::test_new("notes");
        notes.cached_git_branch = None;
        app.workspaces = vec![main, issue, notes];
        app.active = Some(0);
        app.selected = 0;
        app.mode = Mode::Terminal;
        app.ensure_test_terminals();

        // Pin the entry variant sequence.
        let entries = workspace_list_entries(&app);
        let variants: Vec<&str> = entries
            .iter()
            .map(|entry| match entry {
                WorkspaceListEntry::GroupHeader { .. } => "GroupHeader",
                WorkspaceListEntry::ProjectHeader { .. } => "ProjectHeader",
                WorkspaceListEntry::BranchHeader { .. } => "BranchHeader",
                WorkspaceListEntry::Workspace { .. } => "Workspace",
                WorkspaceListEntry::HiddenHeader { .. } => "HiddenHeader",
            })
            .collect();
        assert_eq!(
            variants,
            ["ProjectHeader", "Workspace", "Workspace", "Workspace",]
        );

        // Height pass: total rows from the shared per-entry height helper.
        let total_height: u16 = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| entry_row_height(entry, &entries, idx))
            .sum();
        assert_eq!(total_height, 4, "1+1+1+1 rows for the pinned sequence");

        // Visible-count pass agrees: a body exactly `total_height` rows tall
        // shows every entry; one row less drops exactly the last (1-row)
        // entry. Section area height = body + header rows + footer row +
        // the always-on "+ run command…" programs row.
        let exact = Rect::new(0, 0, 30, total_height + WORKSPACE_SECTION_HEADER_ROWS + 2);
        assert_eq!(workspace_list_visible_count(&app, exact, 0), entries.len());
        let short = Rect::new(0, 0, 30, total_height + WORKSPACE_SECTION_HEADER_ROWS + 1);
        assert_eq!(
            workspace_list_visible_count(&app, short, 0),
            entries.len() - 1
        );

        // Geometry pass agrees: card/header rects sit at the prefix sums of
        // `entry_row_height` when the body is tall enough for everything.
        let sidebar = Rect::new(0, 0, 30, 40);
        let (cards, headers) = compute_workspace_list_areas(&app, sidebar);
        let ws_area = workspace_list_rect(sidebar, app.sidebar_section_split);
        let body = workspace_list_body_rect(&app, ws_area, false);
        let mut expected_card_ys = Vec::new();
        let mut expected_header_ys = Vec::new();
        let mut y = body.y;
        for (idx, entry) in entries.iter().enumerate() {
            match entry {
                WorkspaceListEntry::Workspace { .. } => expected_card_ys.push(y),
                WorkspaceListEntry::GroupHeader { .. }
                | WorkspaceListEntry::ProjectHeader { .. } => expected_header_ys.push(y),
                WorkspaceListEntry::BranchHeader { .. } => {}
                WorkspaceListEntry::HiddenHeader { .. } => {}
            }
            y += entry_row_height(entry, &entries, idx);
        }
        assert_eq!(y - body.y, total_height);
        assert_eq!(
            cards.iter().map(|card| card.rect.y).collect::<Vec<_>>(),
            expected_card_ys
        );
        assert_eq!(
            cards.iter().map(|card| card.ws_idx).collect::<Vec<_>>(),
            [0, 1, 2]
        );
        assert_eq!(
            headers
                .iter()
                .map(|header| header.rect.y)
                .collect::<Vec<_>>(),
            expected_header_ys
        );

        // Render pass agrees: labels land on the same prefix-sum rows in an
        // exact-fit section area.
        let mut terminal =
            Terminal::new(TestBackend::new(exact.width, exact.height)).expect("test terminal");
        let runtimes = TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, exact, false))
            .expect("workspace list should render");
        let row_text = |row: u16| -> String {
            (0..exact.width)
                .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                .collect()
        };
        let body_y = WORKSPACE_SECTION_HEADER_ROWS; // exact rect starts at y = 0
        assert!(
            row_text(body_y).contains("herdr"),
            "project header row: {:?}",
            row_text(body_y)
        );
        assert!(
            row_text(body_y + 3).contains("notes"),
            "flat workspace card row: {:?}",
            row_text(body_y + 3)
        );

        // Bracket-rail prefixes: header opens with ╭─, the folded main member
        // rides the spine (│), and the last member closes it (╰──).
        assert!(
            row_text(body_y).contains("╭─"),
            "project header opens bracket: {:?}",
            row_text(body_y)
        );
        assert!(
            row_text(body_y + 1).contains('│'),
            "folded member on spine: {:?}",
            row_text(body_y + 1)
        );
        assert!(
            row_text(body_y + 2).contains("╰──"),
            "last member closes bracket: {:?}",
            row_text(body_y + 2)
        );

        // Invariants gate for the state used above, so later field additions
        // keep passing through this check.
        app.assert_invariants_for_test();
        for ws in &app.workspaces {
            ws.assert_invariants_for_test();
        }
    }

    #[test]
    fn hiding_workspace_moves_it_to_hidden_section() {
        let mut app = AppState::test_new();
        let mut a = Workspace::test_new("alpha");
        a.cached_git_branch = None;
        let mut b = Workspace::test_new("beta");
        b.cached_git_branch = None;
        app.workspaces = vec![a, b];
        let key = AppState::workspace_hide_key(&app.workspaces[0]);
        app.hidden_space_keys.insert(
            key,
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        );

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::HiddenHeader { count: 1 },
            ]
        );
    }

    #[test]
    fn expanded_hidden_section_emits_hidden_rows() {
        let mut app = AppState::test_new();
        let mut a = Workspace::test_new("alpha");
        a.cached_git_branch = None;
        let mut b = Workspace::test_new("beta");
        b.cached_git_branch = None;
        app.workspaces = vec![a, b];
        let key = AppState::workspace_hide_key(&app.workspaces[0]);
        app.hidden_space_keys.insert(
            key,
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        );
        app.hidden_section_expanded = true;

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::HiddenHeader { count: 1 },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn expired_hide_is_not_applied() {
        let mut app = AppState::test_new();
        let mut a = Workspace::test_new("alpha");
        a.cached_git_branch = None;
        let mut b = Workspace::test_new("beta");
        b.cached_git_branch = None;
        app.workspaces = vec![a, b];
        let key = AppState::workspace_hide_key(&app.workspaces[0]);
        app.hidden_space_keys.insert(
            key,
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                    rail: BranchRail::None,
                },
            ]
        );
    }

    #[test]
    fn hiding_visual_group_hides_members_and_header() {
        let mut app = AppState::test_new();
        let mut ws0 = Workspace::test_new("alpha");
        ws0.visual_group = Some("g1".into());
        ws0.cached_git_branch = None;
        let mut ws1 = Workspace::test_new("beta");
        ws1.visual_group = Some("g1".into());
        ws1.cached_git_branch = None;
        app.workspaces = vec![ws0, ws1];
        app.hidden_space_keys.insert(
            "vg:g1".to_string(),
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        );

        assert_eq!(
            workspace_list_entries(&app),
            vec![WorkspaceListEntry::HiddenHeader { count: 2 }]
        );
    }
}
