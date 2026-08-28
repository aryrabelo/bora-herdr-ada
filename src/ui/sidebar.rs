#[cfg(test)]
mod capture;
pub(crate) mod project_view;
// Render wiring landed for `PaneDotsRow` (F2) and for the branch header
// (F3, bora-79l T3): `project_view` now consumes `Section.header_on`,
// `SectionParts.diff` and the `SectionKind::Branch` shape at emission, and
// `section_row_line` renders the declared header. Still unconsumed:
// `SectionParts.dots` (the l2 toggle) and full model-driven section
// emission (F7) — dead_code stays allowed until those land.
#[allow(dead_code)]
pub(crate) mod sections;
mod tokens;

use std::time::Instant;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use self::tokens::{ResolvedToken, ResolvedTokenKind, SpaceTokenContext};
use super::scrollbar::{render_scrollbar, should_show_scrollbar};
use super::status::{
    agent_icon, blocked_glyph, format_idle_age, idle_age_color, state_dot, state_label,
    state_label_color,
};
use super::text::{display_width, display_width_u16, truncate_end};
use crate::app::state::{AgentPanelSort, Palette, ProjectRowHitArea, ProjectRowTarget};
use crate::app::{AppState, Mode};
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

const AGENT_PANEL_HEADER_ROWS: u16 = 3;
/// Blank row reserved above the first workspace-list entry. Not a header: the
/// drag-reorder "drop above the first card" indicator needs a terminal row of
/// its own (every other insert slot renders at `card.y - 1`; without this row
/// the first card would sit at y=0 and that slot would have nowhere to draw
/// or be hit-tested). Doubles as the list's top margin.
const WORKSPACE_LIST_TOP_MARGIN_ROWS: u16 = 1;

/// Glyph + style for a resolved `ChecksRollup` value, shared by
/// `checks_badge` (worktree/branch PR badges) and `pr_checks_glyph`
/// (bora-yw6.2's PULL REQUESTS band rows) so the CHECKS palette never
/// drifts between the two surfaces.
fn checks_rollup_glyph(
    rollup: crate::workspace::ChecksRollup,
    p: &Palette,
) -> (&'static str, Style) {
    use crate::workspace::ChecksRollup;
    match rollup {
        ChecksRollup::Passing => (" ✓", Style::default().fg(p.green)),
        ChecksRollup::Failing => (" ✗", Style::default().fg(p.red)),
        ChecksRollup::Pending => (" ●", Style::default().fg(p.yellow)),
    }
}

/// Glyph + style for a PR's rolled-up check status, shown after the PR badge.
fn checks_badge(
    checks: &[crate::workspace::CheckRun],
    p: &Palette,
) -> Option<(&'static str, Style)> {
    Some(checks_rollup_glyph(
        crate::workspace::checks_rollup(checks)?,
        p,
    ))
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

/// Whether the agent-detail panel occupies the bottom of the expanded sidebar.
///
/// Retired (2026-08-23): the panel is visually gone so the Project view's three
/// levels get the whole column. The panel code is deliberately NOT deleted —
/// every `agent_panel_*` helper keeps its existing call sites and simply runs
/// against a zero-height rect, so nothing became dead code, no `AppState`
/// field was dropped, and old snapshots (`sidebar_section_split`) restore
/// unchanged. Flip this to `true` to bring the panel back.
const AGENT_PANEL_VISIBLE: bool = false;

pub(crate) fn expanded_sidebar_sections(area: Rect, split_ratio: f32) -> (Rect, Rect) {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height == 0 {
        return (Rect::default(), Rect::default());
    }

    let (ws_h, detail_h) = if AGENT_PANEL_VISIBLE {
        sidebar_section_heights(content.height, split_ratio)
    } else {
        (content.height, 0)
    };
    let ws_area = Rect::new(content.x, content.y, content.width, ws_h);
    let detail_area = Rect::new(content.x, content.y + ws_h, content.width, detail_h);
    (ws_area, detail_area)
}

pub(crate) fn sidebar_section_divider_rect(area: Rect, split_ratio: f32) -> Rect {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if !AGENT_PANEL_VISIBLE || content.width == 0 || content.height < 6 {
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

/// Right-aligned click target on the workspace list's top margin row that
/// cycles Flat/Repo/Project view (bora regression fix: commit 7bb8133b
/// removed both the ` spaces` title and this toggle when it only meant to
/// drop the title — restoring the toggle alone, not the title).
pub(crate) fn view_mode_toggle_rect(area: Rect, mode: crate::config::ViewMode) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }

    let label = mode.as_str();
    let width = display_width_u16(label).min(area.width);
    Rect::new(area.x + area.width.saturating_sub(width), area.y, width, 1)
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
                std::cmp::Reverse(crate::detect::attention_priority(entry.state, entry.seen)),
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
                .max_by_key(|(s, seen)| crate::detect::display_priority(*s, *seen))
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

/// First pane's agent identity for a workspace's tree row, e.g. the ` @nome`
/// badge. A registered `bora agent rename` name wins; a pane with only a
/// DETECTED agent falls back to its addressable pane id (`w78p1`) — the agent
/// kind ("omp", "pi") names a tool, not an agent, while the pane id is what
/// `bora agent prompt`/`orc channel send` actually accept (unpunctuated form
/// resolves identically to `w78:p1`). Pure in-memory lookup, safe per render.
fn workspace_agent_label(
    ws: &crate::workspace::Workspace,
    terminals: &std::collections::HashMap<
        crate::terminal::TerminalId,
        crate::terminal::TerminalState,
    >,
) -> Option<String> {
    let detail = ws.pane_details(terminals).into_iter().next()?;
    let registered = ws
        .tabs
        .iter()
        .find_map(|tab| tab.panes.get(&detail.pane_id))
        .and_then(|pane| terminals.get(&pane.attached_terminal_id))
        .and_then(|terminal| terminal.agent_name.clone());
    Some(registered.unwrap_or_else(|| {
        ws.public_pane_number(detail.pane_id)
            .map(|n| format!("{}p{}", ws.id, crate::workspace::encode_public_number(n)))
            // No public pane number (shouldn't happen for a live pane): the
            // detected kind still beats an empty badge.
            .unwrap_or(detail.agent_label)
    }))
}

/// Label for an indented (child) workspace row. The repo-derived display
/// name is not unique there — two workspaces on the same checkout and
/// branch render the same string — so the row identifies itself by the
/// `@wNpN` pane badge the Workspace arm draws beside this label, plus the
/// branch only when the parent header did not already print it. A custom
/// name is the user's own label and passes through verbatim.
fn indented_child_label(ws: &crate::workspace::Workspace, parent_branch: Option<&str>) -> String {
    if let Some(name) = ws.custom_name.as_deref() {
        return name.to_string();
    }
    match ws
        .branch()
        .map(|branch| branch_display_label(&branch).to_string())
    {
        Some(label) if parent_branch != Some(label.as_str()) => label,
        _ => String::new(),
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
        .max_by_key(|(state, seen)| crate::detect::display_priority(*state, *seen))
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

/// Where a Project-view attachment band may appear in the tree: hanging off
/// a `WorktreeRow` (today: COMMANDS, CHECKS) or off a `ProjectRow` (today:
/// TODOS, NOTES, PULL REQUESTS). A descriptor declares its own level, and
/// the resolver (`project_view::filter_by_level`) honours it — this is what
/// makes placing a band where it never declared it may appear
/// unrepresentable, rather than merely unconventional (bora-by6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionLevel {
    Worktree,
    Project,
}

/// How a band header's right-aligned counter reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionCounter {
    /// `done/total`, a progress readout (COMMANDS, CHECKS, TODOS).
    Progress,
    /// A plain count with no denominator — a list, not a progress bar
    /// (NOTES, PULL REQUESTS).
    Count,
}

/// How a band's item bullet reacts to `running`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionBullet {
    /// `●` running / `·` idle — the default.
    Standard,
    /// Idle renders as a red `✗` failure marker instead of a dim dot: CHECKS
    /// rows exist only to flag failures, so an idle row IS the problem.
    FlagIdleAsError,
}

/// Registry entry for one Project-view attachment band (bora-by6). This
/// replaces the closed `ProjectSection` enum: placement (`level`),
/// presentation (`glyph`/`label`/`counter`/`bullet`), and behavior (`push`)
/// for a band all live on its descriptor instead of being scattered across
/// per-variant match arms, so a new band costs one `const` registry entry
/// (`project_view::REGISTRY`) plus one push function — not a match arm in
/// every site that used to enumerate the closed set. A row band is referred
/// to by `&'static` reference to a descriptor, never by enum variant.
#[derive(Debug)]
pub(crate) struct SectionDescriptor {
    /// The name a `sections.order:` entry uses (case-insensitive lookup via
    /// `from_wire_name`).
    pub(crate) wire_name: &'static str,
    pub(crate) glyph: &'static str,
    pub(crate) label: &'static str,
    pub(crate) level: SectionLevel,
    pub(crate) counter: SectionCounter,
    pub(crate) bullet: SectionBullet,
    /// Pushes this band's header/items onto `entries`, or nothing when the
    /// band's data source is empty/absent (rule 5) — and an explicit error
    /// row, never a silently empty band, when the data source errored. The
    /// seven pre-registry push functions had non-uniform signatures
    /// (`&[String]` selection, `&Workspace`, a bare `slug`, an added
    /// `local_branches`); `project_view::SectionPushCtx` normalizes them
    /// onto one borrowed-context type so one function-pointer type fits
    /// every band.
    pub(crate) push: fn(&mut Vec<WorkspaceListEntry>, &project_view::SectionPushCtx<'_>),
}

// `wire_name` is the unique key by design (one registry entry per name,
// checked in `#[test] fn registry_wire_names_are_unique`), so equality
// compares it alone. A derived `PartialEq` would also compare `push`, and
// comparing function pointers for equality is unreliable across codegen
// units (rustc's `unpredictable_function_pointer_comparisons` lint) — this
// impl sidesteps that entirely rather than silencing the lint.
impl PartialEq for SectionDescriptor {
    fn eq(&self, other: &Self) -> bool {
        self.wire_name == other.wire_name
    }
}
impl Eq for SectionDescriptor {}

impl SectionDescriptor {
    /// Case-insensitive `sections.order:` lookup. `None` for an
    /// unrecognized name — the resolver ignores it rather than erroring, so
    /// a future bora writing an unknown section name into `projects.yml`
    /// cannot break an older binary's sidebar.
    pub(crate) fn from_wire_name(name: &str) -> Option<&'static SectionDescriptor> {
        project_view::REGISTRY
            .iter()
            .copied()
            .find(|section| section.wire_name.eq_ignore_ascii_case(name))
    }
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

    // ── Project view (`ViewMode::Project`) ────────────────────────────────
    // These six variants are emitted ONLY by the project-view builder in
    // `sidebar::project_view`; the Flat and Repo views never produce them and
    // are untouched. Every one is height 1, like every variant above.
    /// Top level: a user-declared project from `projects.yml`. `live`/`total`
    /// is the aggregate workspace count rendered right-aligned. The trailing
    /// implicit group holding workspaces that match no member has
    /// `declared: false`.
    ProjectRow {
        name: String,
        collapse_key: String,
        live: usize,
        total: usize,
        declared: bool,
    },
    /// A worktree found on disk with no workspace open on it (bora-qdi):
    /// rendered dimmed as an open affordance, carries no `ws_idx` children.
    /// This is the ONLY case this variant still covers (bora-c1h) — every
    /// OPEN checkout now renders one `SectionRow` per workspace instead
    /// (`repo`/`ahead`/`behind`/`pr` stay meaningless for an unopened row
    /// and are always the zero/`None` defaults).
    WorktreeRow {
        checkout_key: String,
        repo: Option<String>,
        branch: String,
        ahead: usize,
        behind: usize,
        pr: Option<u64>,
        collapse_key: String,
        unopened: bool,
    },
    /// T6 pass 6a (bora-79l.10): the GROUP row of one branch section —
    /// ONE `SectionRow` per branch group (`branch_group`), header at the
    /// TOP of the group, the members' `PaneDotsRow` blocks contiguous
    /// below. Before 6a every workspace got its own `SectionRow` and the
    /// same-branch exception pushed the one visible header BETWEEN the
    /// blocks (the "generic-row problem" this bead exists to kill). No
    /// new variant carries the change: this row, `PaneDotsRow` and
    /// `SectionHeader` are the runtime.
    ///
    /// Per-workspace fields name the REPRESENTATIVE member (the FIRST
    /// workspace of the group): `ws_idx` is the workspace git/PR/checks
    /// state is read from at render time (see `section_row_line`),
    /// `checkout_key` names its checkout (bora-uqv's
    /// `ProjectMemberTargets` right-click menu resolves `member_dir`
    /// straight from it), and `collapse_key` (`wsec:{ws_idx}`)
    /// collapses the whole group's blocks — one toggle per section now.
    SectionRow {
        ws_idx: usize,
        checkout_key: String,
        collapse_key: String,
        /// T3 (bora-79l): the section model's header switch — read from the
        /// project's `layout:` at emission (`section_model_flags`), obeyed
        /// by the renderer and the geometry pass. T6's toggle button WRITES
        /// the model; this field only carries it.
        header_on: bool,
        /// Same-branch exception (T3 decision 5): set at emission when a
        /// LOWER `SectionRow` of the same (repo, branch) exists in this
        /// project group — the upper header stays hidden so two headers of
        /// one branch never coexist visible. 6a narrowed where that can
        /// happen: emission groups by `branch_group`, so within one
        /// project the exception only fires between STACKED runtime
        /// sections (rare, 6b's world); in the normal one-section-per-
        /// branch shape the header simply sits at the top.
        header_hidden: bool,
        /// T3: `SectionParts.diff` from the same model lookup — gates the
        /// `+N −M` diff numbers inside the header's state cluster.
        show_diff: bool,
        /// T7 (bora-79l, divergence C): the branch-GROUP key
        /// (`project_view::branch_group_key` — repo identity + branch)
        /// the whole container is keyed by. `project_view_trailing_gap`
        /// compares it across consecutive sections to place the blank
        /// separator row: blank BETWEEN branch groups (and before a band
        /// header), never between sibling workspaces of one branch
        /// (ALVO_CAPTURE rows 04-07 are glued, row 08 is blank).
        branch_group: String,
        /// 6a: the group's `+N −M` cluster — the SUM of every member's
        /// cached change set, folded once at emission (membership is
        /// emission-time knowledge the renderer does not have); the
        /// renderer only gates it on `show_diff`. PR/checks stay the
        /// representative's own (`ws_idx` above).
        diff: Option<(u32, u32)>,
    },
    /// Third level: a `COMMANDS` or `CHECKS` band hanging off a worktree,
    /// with a right-aligned `done/total`. Emitted only when non-empty, in
    /// declared order (`sections.order:`, default COMMANDS then CHECKS —
    /// bora-5ia, `project_view::resolve_section_order`).
    /// `TODOS`/`NOTES` use the same shape one level up, hanging off the
    /// project row (bora-s3y.3), also declarable, default TODOS then NOTES.
    SectionHeader {
        kind: &'static SectionDescriptor,
        collapse_key: String,
        done: usize,
        total: usize,
        /// T6 6b (bora-79l.10): `Section.name`, when declared, overrides
        /// the descriptor's static `label` at render (`section_header_
        /// line`) — the only way a per-instance header name can reach
        /// this row, since `kind` is a shared `&'static` descriptor and
        /// cannot itself carry one (no `Box::leak`, plan decision).
        /// `None` for every registry band (COMMANDS/CHECKS/TODOS/
        /// NOTES/PULL REQUESTS): they have no per-instance name to
        /// carry, only declared (`push_declared_sections`) sections do.
        name: Option<String>,
    },
    /// A row inside a `SectionHeader` band.
    SectionItem {
        kind: &'static SectionDescriptor,
        label: String,
        detail: Option<String>,
        running: bool,
        /// The workspace a COMMANDS row launches into (the worktree's
        /// representative workspace, bora-55c.3). `None` for bands whose
        /// rows are not launchable (CHECKS/TODOS/NOTES).
        ws_idx: Option<usize>,
    },
    /// Project view's replacement for `PaneRow`: one 2-line BLOCK per OPEN
    /// workspace, regardless of pane count — l1 the workspace's own
    /// unique name (`pane_dots_name_line`), l2 one state dot per pane
    /// (`pane_dots_dots_line`), height 2 (`entry_row_height`, bora-79l F2,
    /// ALVO_CAPTURE rows 04-05/28-29). `name` is already disambiguated at
    /// emission (the same unique-name resolution `SectionRow`'s sibling
    /// `Workspace` rows use elsewhere), so this variant never re-derives
    /// it.
    ///
    /// Pane identity AND pane state are both read from
    /// `AppState.workspaces[ws_idx]` at render/hit-test time, never
    /// carried on the entry — same rule `SectionRow` already states for
    /// git/PR state, extended here to the pane list itself since its
    /// length is not fixed. This is why `workspace_list_areas_for_entries`
    /// (unlike every other arm there) takes an `app: &AppState` parameter:
    /// per-dot hit areas need each pane's live `pane_id` to build a
    /// `ProjectRowTarget::Pane`, and a 2-field entry has nowhere else to
    /// get it from. Dot hit areas land on l2 (`row_y + 1`), never l1.
    PaneDotsRow {
        ws_idx: usize,
        name: String,
        /// T6 6b (bora-79l.10): the owning Branch section's
        /// `parts.dots` flag (`project_view::section_model_flags`),
        /// carried onto the entry so every lockstep pass reads the
        /// l2-toggle off the row itself rather than re-deriving it from
        /// the model a second time. OFF collapses the block to its l1
        /// name line alone (`entry_row_height`).
        dots: bool,
    },
    /// Project-level: one open PR authored by the user with no local
    /// worktree, inside the `PULL REQUESTS` band (bora-yw6.2, C2). `checks`
    /// is A1's `OpenPr.checks: Option<ChecksRollup>` (`workspace::git::open_prs`
    /// reuses the pre-existing `check_status::ChecksRollup` rather than a
    /// second convention — `None` means no checks reported for the head
    /// commit), read straight off `AppState.repo_open_prs` — no fetch, no
    /// allocation beyond the fields themselves, same as every other row
    /// here. Height 1.
    PrRow {
        number: u64,
        title: String,
        url: String,
        head_ref: String,
        is_draft: bool,
        checks: Option<crate::workspace::ChecksRollup>,
        /// A representative workspace of this PR's repo, resolved ONCE when
        /// the band is built, so the click can name which repo to create the
        /// worktree in. `None` when no open workspace shares the repo
        /// identity yet, which makes the row render normally but stay
        /// un-clickable rather than opening a worktree in the wrong repo.
        /// Resolved at build time and not at hit-test time on purpose: the
        /// lookup is a scan over `app.workspaces`, and the geometry walk runs
        /// per render x per pane x per client.
        ws_idx: Option<usize>,
    },
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
    entries: &[WorkspaceListEntry],
    idx: usize,
    row_gap: u16,
) -> u16 {
    let base: u16 = match entry {
        WorkspaceListEntry::GroupHeader { .. } => 1,
        WorkspaceListEntry::ProjectHeader { .. } => 1,
        WorkspaceListEntry::BranchHeader { .. } => 1,
        WorkspaceListEntry::Workspace { .. } => 1,
        WorkspaceListEntry::HiddenHeader { .. } => 1,
        WorkspaceListEntry::ProjectRow { .. } => 1,
        WorkspaceListEntry::WorktreeRow { .. } => 1,
        WorkspaceListEntry::SectionRow { .. } => 1,
        WorkspaceListEntry::SectionHeader { .. } => 1,
        WorkspaceListEntry::SectionItem { .. } => 1,
        // bora-79l F2: the block split into l1 (name) + l2 (dots) —
        // `pane_dots_name_line`/`pane_dots_dots_line`'s own docs.
        WorkspaceListEntry::PaneDotsRow { dots, .. } => {
            if *dots {
                2
            } else {
                1
            }
        }
        WorkspaceListEntry::PrRow { .. } => 1,
    };
    base + project_view_trailing_gap(entry, entries, idx, row_gap)
}

/// Trailing blank-row discipline for Project view (bora-c1h G7, T7
/// bora-79l divergence C, T6 6a the group shape). Two rules:
///
/// - After a `PaneDotsRow`: a blank row separates the END of a branch
///   GROUP (or the last group, before a band) from the next header —
///   never sibling members of one branch (ALVO_CAPTURE: rows 04-07
///   glued, row 08 blank), never right before the next project's
///   `ProjectRow`, never after the final block in the list. "Same
///   group" is decided by `SectionRow::branch_group` (repo identity +
///   branch): a block's own group is the nearest `SectionRow` ABOVE it
///   — 6a made groups hold 2+ blocks, so the walk skips sibling
///   `PaneDotsRow`s instead of reading `entries[idx - 1]` directly. A
///   next section whose header is HIDDEN (stacked-sections same-branch
///   exception) suppresses the gap too: the hidden header still owns a
///   row and paints nothing, so that row already IS the separator — a
///   gap on top of it was the doubled blank the owner pointed at.
/// - After a `ProjectRow` (6a): one blank "respiro" between the group
///   header and its first content (ALVO_CAPTURE row 02, the anatomy's
///   "Section · tipo LIVRE — respiro após o grupo"). Fires before a
///   section header or a band header, never before another project,
///   the unopened rows' boundary implied by their own separator rule,
///   or the end of a collapsed group (nothing was pushed below).
///
/// Attribution: before T7 the gap fired after EVERY block; before
/// bora-c1h G7 `PaneRow` could repeat N times per workspace, so the gap
/// only applied after the LAST sibling `PaneRow` of a block.
///
/// `entry_row_height`'s own `entries`/`idx` peek (its doc) is exactly what
/// this needs, so the three lockstep passes stay in agreement by
/// construction — no separate pass.
fn project_view_trailing_gap(
    entry: &WorkspaceListEntry,
    entries: &[WorkspaceListEntry],
    idx: usize,
    row_gap: u16,
) -> u16 {
    if matches!(entry, WorkspaceListEntry::ProjectRow { .. }) {
        return match entries.get(idx + 1) {
            Some(WorkspaceListEntry::SectionRow { .. })
            | Some(WorkspaceListEntry::SectionHeader { .. }) => row_gap,
            _ => 0,
        };
    }
    let WorkspaceListEntry::PaneDotsRow { .. } = entry else {
        return 0;
    };
    // The owning section: the nearest `SectionRow` above this block.
    // Emission guarantees one exists (the group header tops every
    // group); `unwrap_or("")` keeps a hand-built orphan block
    // conservative — a separator, never a glue.
    let own_group = entries[..idx]
        .iter()
        .rev()
        .find_map(|e| match e {
            WorkspaceListEntry::SectionRow { branch_group, .. } => Some(branch_group.as_str()),
            _ => None,
        })
        .unwrap_or("");
    match entries.get(idx + 1) {
        None => 0,
        Some(WorkspaceListEntry::ProjectRow { .. }) => 0,
        // The next member block of the SAME group: glued (6a — a group
        // is one section, its blocks contiguous under the header).
        Some(WorkspaceListEntry::PaneDotsRow { .. }) => 0,
        // A hidden header (stacked-sections exception) paints nothing —
        // its OWNED row already IS the blank separator, so the gap
        // would double it (T7 divergence C: "hoje há branco dobrado em
        // alguns lugares" was exactly gap + hidden-header row).
        Some(WorkspaceListEntry::SectionRow {
            header_hidden: true,
            ..
        }) => 0,
        Some(WorkspaceListEntry::SectionRow {
            branch_group: next, ..
        }) if next == own_group => 0,
        _ => row_gap,
    }
}

/// Chevron glyph for a collapsible Project-view row (`ProjectRow`,
/// `WorktreeRow`). Shared so the two variants' chevrons never drift.
fn project_chevron(collapsed: bool) -> &'static str {
    if collapsed {
        "▸"
    } else {
        "▾"
    }
}

/// Compose a Project-view row from left-hand spans plus a right-aligned
/// trailing span, filling the gap between them with `fill` (plain spaces
/// when `None`, a ruler character when `Some`). Centralizes the width
/// arithmetic: `ProjectRow`'s `n/m` and `SectionHeader`'s ruled `n/m` both
/// go through this, so the left and right budgets can't drift out of sync —
/// forgetting to reserve the trailing width in one place while truncating
/// in another is exactly the truncation-budget bug this session's sidebar
/// work has already shipped twice.
fn project_row_trailing(
    mut spans: Vec<Span<'static>>,
    trailing: Span<'static>,
    fill: Option<(char, Style)>,
    width: u16,
) -> Line<'static> {
    let width = width as usize;
    let used: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    let trailing_width = display_width(trailing.content.as_ref());
    let gap = width.saturating_sub(used + trailing_width);
    if gap > 0 {
        match fill {
            Some((ch, style)) => spans.push(Span::styled(ch.to_string().repeat(gap), style)),
            None => spans.push(Span::styled(" ".repeat(gap), Style::default())),
        }
    }
    spans.push(trailing);
    Line::from(spans)
}

/// Top-level Project-view row: one leading gutter column, an optional
/// chevron, project name, `n/m` (live/total workspaces) right-aligned.
/// See `WorkspaceListEntry::ProjectRow`. T7 (bora-79l, divergence F): the
/// title renders ` Bora` with one leading space (ALVO_CAPTURE row 01) —
/// the same single gutter column every other Project-view row starts with.
///
/// Chevron only when CLOSED (owner's later ask, on top of ground-truth
/// re-approval's original "no chevron at all"): an expanded group already
/// shows its own workspace rows below, each carrying its own `SectionRow`
/// `▾`/`▸` disclosure glyph, so a second one here would be a duplicate
/// affordance the approved mock never drew. A collapsed group shows
/// nothing else beneath it, so it gets the caret back to say so.
///
/// Still no ruler either way (ground-truth re-approval, pinned by
/// `project_row_line_has_no_separator_rule_before_the_counter`): the
/// approved mock's `.g` rule draws none — the underline alone reads as a
/// header. Solo #11's dash-fill ruler (kept by `section_header_line`, a
/// distinct third-level row) was a deviation from the approved design, not
/// the source of truth, and stays rejected here — the gap before the
/// counter is plain space.
pub(crate) fn project_row_line(
    name: &str,
    live: usize,
    total: usize,
    collapsed: bool,
    p: &Palette,
    width: u16,
) -> Line<'static> {
    let counter = format!(" {live}/{total}");
    // T7 divergence F: the 1-column gutter every Project-view row shares.
    let mut spans = vec![Span::styled(" ", Style::default())];
    if collapsed {
        spans.push(Span::styled(
            format!("{} ", project_chevron(true)),
            Style::default().fg(p.overlay1),
        ));
    }
    let prefix_width: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    // +1 reserves the mandatory separating space after the name so a long
    // name never butts directly against the counter.
    let avail = (width as usize).saturating_sub(prefix_width + display_width(&counter) + 1);
    spans.push(Span::styled(
        truncate_end(name, avail),
        // ITALIC | UNDERLINED, no BOLD (item 6, owner's decision after a
        // channel-collision check): a terminal only offers THREE face
        // channels — bold, italic, bold-italic (Ghostty's
        // `font-family-{bold,italic,bold-italic}` / `font-style-*`).
        // ITALIC alone claims the plain-italic channel for a display face,
        // uncontested. `section_row_line`'s branch label already claims
        // BOLD|ITALIC for ITS OWN distinct face (see that span's own
        // "don't clean this up" comment) — putting this header on
        // BOLD|ITALIC too would repaint every branch label in Project view
        // along with it, defeating the point of a face aimed at only this
        // row. BOLD is deliberately absent for a second reason: this row's
        // own slightly-lighter background (`p.surface0`, painted by the
        // `ProjectRow` render arm) now supplies the emphasis BOLD used to
        // carry — the owner's own call ("I don't think we even need the
        // Bold if we had the background").
        Style::default()
            .fg(p.mauve)
            .add_modifier(Modifier::ITALIC | Modifier::UNDERLINED),
    ));
    spans.push(Span::styled(" ", Style::default()));
    project_row_trailing(
        spans,
        // The counter stays dim (`p.overlay0`). It briefly used `p.red` to
        // supply a "pink" companion to the ask "half purple, half pink",
        // reasoning that Catppuccin's `red` is a soft rose and that this row
        // carries no state cluster to collide with. Reverted: the harm the
        // binding rule names ("spending red on it makes a real CI failure
        // harder to spot") is about the READER's eye scanning the sidebar
        // for red, not about collisions within one row — putting a rose tone
        // on every project header trains that eye to ignore the hue. The ask
        // needs no second colour anyway: `p.mauve` is `Rgb(203, 166, 247)`,
        // purple leaning pink, which IS "half purple, half pink" in one
        // swatch. Adding a real `pink` palette field stays available if the
        // owner later wants two distinct tones here.
        //
        // Investigated per the lead's ask, and worth keeping: `p.mauve` is a
        // real, distinct colour in `Palette::catppuccin()` and in every
        // RGB-capable built-in theme, and `project_row_trailing`'s later
        // spans never touch an earlier span's fg (each `Span` keeps its own
        // style), so there is no override bug here.
        Span::styled(counter, Style::default().fg(p.overlay0)),
        None,
        width,
    )
}

/// Second-level Project-view row, unopened worktrees ONLY now (bora-c1h):
/// chevron, optional repo name (omitted when the project holds a single
/// repo), branch, ahead/behind, PR badge, always dimmed — a worktree found
/// on disk with no workspace open on it, an open affordance rather than a
/// live row. Every OPEN checkout renders via `section_row_line` instead.
pub(crate) fn worktree_row_line(
    repo: Option<&str>,
    branch: &str,
    ahead: usize,
    behind: usize,
    pr: Option<u64>,
    collapsed: bool,
    unopened: bool,
    p: &Palette,
    width: u16,
) -> Line<'static> {
    let dim = |style: Style| {
        if unopened {
            style.add_modifier(Modifier::DIM)
        } else {
            style
        }
    };
    let mut spans = vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            project_chevron(collapsed),
            dim(Style::default().fg(p.accent)),
        ),
        Span::styled(" ", Style::default()),
    ];
    if let Some(repo) = repo {
        spans.push(Span::styled(
            format!("{repo}  "),
            dim(Style::default().fg(p.overlay0)),
        ));
    }
    let mut trailing: Vec<Span<'static>> = Vec::new();
    if ahead > 0 {
        trailing.push(Span::styled(" ", Style::default()));
        trailing.push(Span::styled(
            format!("↑{ahead}"),
            dim(Style::default().fg(p.green)),
        ));
    }
    if behind > 0 {
        trailing.push(Span::styled(" ", Style::default()));
        trailing.push(Span::styled(
            format!("↓{behind}"),
            dim(Style::default().fg(p.red)),
        ));
    }
    if let Some(pr) = pr {
        trailing.push(Span::styled(" ", Style::default()));
        trailing.push(Span::styled(
            format!("#{pr}"),
            dim(Style::default().fg(p.green)),
        ));
    }
    let prefix_width: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    let trailing_width: usize = trailing
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    let avail = (width as usize).saturating_sub(prefix_width + trailing_width);
    spans.push(Span::styled(
        truncate_end(branch, avail),
        dim(Style::default().fg(p.overlay1)),
    ));
    spans.extend(trailing);
    Line::from(spans)
}

/// PR chip tone for `section_row_line` — derived from `PrSummary.state` at
/// the call site so this function stays string-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrChipTone {
    Open,
    Merged,
    Draft,
    Closed,
}

/// Display cap for a git ahead/behind count (G5 ground-truth re-approval):
/// a real fork can sit tens of thousands of commits behind (the owner's own
/// `rails/rails` example, ~99485) and an unbounded integer in the fixed
/// state-cluster budget pushes the whole cluster off the row's right edge.
/// Caps the rendered STRING only — the underlying `usize` this receives is
/// never touched, only how it prints.
fn capped_count(n: usize) -> String {
    if n > 99 {
        "99+".to_string()
    } else {
        n.to_string()
    }
}

/// T3 (bora-79l): the DECLARED branch header of a sessions `Section` —
/// `⎇ main ········································PR42 ✗` (the
/// ALVO_CAPTURE rows 03/09/15/19/23/27 contract; T7 divergence B: the
/// cluster sits FLUSH against the leader — no space after the last dot).
/// Slot order is fixed:
/// `[⌗ linked-worktree marker] [⎇ branch label] [dotted leader]
/// [state cluster]` — no chevron (collapse belongs to the folder,
/// `ProjectRow`) and no workspace/repo name slot: the name lives on the
/// section's `PaneDotsRow` block, which is what killed the P1 redundancy
/// of printing it on both lines.
///
/// Attribution — before T3 this line was `▾ MAIN ⎇ main   PR42 ✗`
/// (chevron + `⌗` in mauve + UPPERCASE repo-name slot with A3's
/// `───────` rule for repeats + a BOLD|ITALIC|DIM branch riding the
/// Ghostty font-selection channel + a cluster of green-ahead /
/// yellow-behind / yellow-dirty glyphs and GitHub-toned PR chips). The
/// owner's model replaced every one of those by assignment: the header
/// declares a BRANCH, the marker is overlay1 (R1: mauve is the
/// ProjectRow's alone), the branch label is plain overlay1+BOLD with no
/// font-selection games, and the whole cluster is R1 gray — red is spent
/// ONLY on a real check failure, through `checks_rollup_glyph`, whose
/// single-owner rule survives untouched. The dirty/staged `✱`/`±` glyph
/// pair is subsumed by the numeric `+N −M` diff (the numbers ARE the
/// state; a glyph beside them would repeat it).
#[allow(clippy::too_many_arguments)] // one row, six independent glyph slots — a struct would only rename this list
pub(crate) fn section_row_line(
    is_worktree: bool,
    branch: Option<&str>,
    diff: Option<(u32, u32)>,
    ahead: usize,
    behind: usize,
    pr: Option<(u64, PrChipTone)>,
    checks: Option<crate::workspace::ChecksRollup>,
    glyphs: &crate::config::ProjectGlyphs,
    p: &Palette,
    width: u16,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![Span::styled(" ", Style::default())];
    if is_worktree {
        spans.push(Span::styled(
            "⌗ ",
            // R1: overlay1, never mauve — mauve on this row would make a
            // linked worktree read as a second folder.
            Style::default().fg(p.overlay1),
        ));
    }
    // Right-aligned state cluster (G5 rule kept): reserve the cluster's
    // full width BEFORE the branch label gets any, so a long label
    // ellipsizes before the cluster ever loses a cell — never the
    // reverse.
    let mut trailing: Vec<Span<'static>> = Vec::new();
    // T7 (bora-79l, divergence B): the cluster sits FLUSH against the
    // dotted leader — a separator space exists only BETWEEN cluster
    // elements, never before the first (ALVO_CAPTURE rows 03/27:
    // `·····PR42 ✗`, `····+916 −2 ↑2 ↓1`). Nested so the push sites
    // below cannot forget the rule.
    fn push_cluster(trailing: &mut Vec<Span<'static>>, span: Span<'static>) {
        if !trailing.is_empty() {
            trailing.push(Span::styled(" ", Style::default()));
        }
        trailing.push(span);
    }
    if let Some((added, removed)) = diff {
        // U+2212 MINUS SIGN, matching ALVO_CAPTURE's `+916 −2` byte for
        // byte; NOT capped like ahead/behind — the alvo itself pins +916.
        push_cluster(
            &mut trailing,
            Span::styled(
                format!("+{added} −{removed}"),
                Style::default().fg(p.overlay1),
            ),
        );
    }
    if ahead > 0 {
        // R1 gray, not green: in the owner's color budget green means
        // "answered/ready" (a pane state). Ahead-of-origin is git
        // plumbing, and spending a loud hue on it would bury the one red
        // that matters (a failing check).
        push_cluster(
            &mut trailing,
            Span::styled(
                format!("{}{}", glyphs.ahead, capped_count(ahead)),
                Style::default().fg(p.overlay1),
            ),
        );
    }
    if behind > 0 {
        // Gray for the same R1 reason the old yellow is gone: being
        // behind origin is a nudge, not a failure, and it must not
        // compete with a real ✗.
        push_cluster(
            &mut trailing,
            Span::styled(
                format!("{}{}", glyphs.behind, capped_count(behind)),
                Style::default().fg(p.overlay1),
            ),
        );
    }
    if let Some((pr, _tone)) = pr {
        // R1: the chip prints gray whatever its state — merged, draft,
        // closed, open. Red is reserved for a failing check (the ✗ glyph
        // after the chip), so `PR42` never shouts. The tone still rides
        // along because `show_checks` below keys on OPEN.
        push_cluster(
            &mut trailing,
            Span::styled(
                format!("{}{pr}", glyphs.pr),
                Style::default().fg(p.overlay1),
            ),
        );
    }
    // Checks glyph has a single owner (repo rule: `run_state` in
    // `workspace/git/check_status.rs` is the only source of a check's
    // rollup state) — always through `checks_rollup_glyph`, never a local
    // color table, so this row and the CHECKS band can never drift on
    // what counts as passing. This is the ONE place in the cluster where
    // red may appear (a real failing check). The glyph only accompanies
    // an OPEN chip (or no chip at all): for merged/closed/draft the CI
    // state is moot and the chip already carries the state. The baked-in
    // leading space is trimmed here — `push_cluster` owns all spacing
    // (T7, divergence B).
    let show_checks = match pr {
        Some((_, tone)) => tone == PrChipTone::Open,
        None => true,
    };
    if show_checks {
        if let Some(rollup) = checks {
            let (glyph, style) = checks_rollup_glyph(rollup, p);
            push_cluster(&mut trailing, Span::styled(glyph.trim_start(), style));
        }
    }

    let prefix_width: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    let trailing_width: usize = trailing
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    // 1 column for the space that separates label from leader; the label
    // budget below excludes the `⎇ ` glyph's own cells, subtracted at the
    // truncate call, and the label ellipsizes into whatever is left
    // (`spike/m0-ambie…` is the mock's own example).
    let branch_glyph_width = branch.map(|_| display_width(glyphs.branch) + 1);
    let label_budget = (width as usize).saturating_sub(prefix_width + trailing_width + 1);
    if let Some(b) = branch {
        if let Some(gw) = branch_glyph_width {
            spans.push(Span::styled(
                format!("{} ", glyphs.branch),
                Style::default().fg(p.overlay1),
            ));
            // A terminal grid has no font-size axis: BOLD alone is the
            // label's emphasis (overlay1 bold, R1 — the old
            // BOLD|ITALIC|DIM font-selection stack died with the name
            // slot; see the attribution above).
            spans.push(Span::styled(
                truncate_end(b, label_budget.saturating_sub(gw)),
                Style::default().fg(p.overlay1).add_modifier(Modifier::BOLD),
            ));
        }
    }
    if trailing_width > 0 {
        // Dotted leader (T3 decision 3): `·` in surface1 — the same
        // connective colour the band headers' own `·` leader uses (T7) —
        // running from the label to the cluster and ending FLUSH against
        // its first element. It exists ONLY when a cluster does; with no
        // cluster there is nothing to lead to, and the row stays as short
        // as its content.
        spans.push(Span::styled(" ", Style::default()));
        let used: usize = spans
            .iter()
            .map(|s| display_width(s.content.as_ref()))
            .sum();
        let dots = (width as usize).saturating_sub(used + trailing_width);
        if dots > 0 {
            spans.push(Span::styled(
                "·".repeat(dots),
                Style::default().fg(p.surface1),
            ));
        }
    }
    spans.extend(trailing);
    Line::from(spans)
}

/// Uncommitted diff totals for a branch header's `+N −M` cluster slot:
/// `added`/`removed` summed over the cached change set's unstaged and
/// staged sections — the same `cached_change_set` the right panel's
/// Changes tab reads, so the two surfaces cannot disagree about what
/// "the diff" is. `None` when nothing counted (clean tree, no cache yet,
/// or binary/untracked-only changes whose numstat is absent).
fn workspace_diff_counts(ws: &crate::workspace::Workspace) -> Option<(u32, u32)> {
    use crate::workspace::ChangeSectionKind;
    let cs = ws.cached_change_set.as_ref()?;
    let mut added = 0u32;
    let mut removed = 0u32;
    for section in &cs.sections {
        match section.kind {
            ChangeSectionKind::Unstaged | ChangeSectionKind::Staged => {
                for file in &section.files {
                    if let (Some(a), Some(r)) = (file.added, file.removed) {
                        added = added.saturating_add(a);
                        removed = removed.saturating_add(r);
                    }
                }
            }
            ChangeSectionKind::Committed => {}
        }
    }
    (added > 0 || removed > 0).then_some((added, removed))
}

/// Third-level Project-view row: a `COMANDO`/`CHECKS` band header — glyph,
/// uppercase name, a `·` dotted leader filling the remaining width, then a
/// right-aligned `done/total` sitting FLUSH against the leader's last dot
/// (T7 bora-79l, ALVO_CAPTURE rows 31/33: ` ≡ COMANDO ·····0/1`). The
/// leader is load-bearing: without it the row reads as a plain label
/// instead of a section; the `·` matches the branch headers' own leader
/// and the indent is the single gutter column every Project-view row
/// shares.
pub(crate) fn section_header_line(
    kind: &'static SectionDescriptor,
    done: usize,
    total: usize,
    // T6 6b (bora-79l.10): `Section.name`, when declared, overrides the
    // descriptor's static `label` — `kind` is a shared `&'static`
    // descriptor and cannot carry a per-instance name itself.
    name: Option<&str>,
    p: &Palette,
    width: u16,
) -> Line<'static> {
    // NOTES/PULL REQUESTS are plain lists, not a progress bar: show the
    // count, not a meaningless `0/N` — a declared field on the descriptor
    // (`SectionCounter`) now, not a wildcard match (bora-by6 G6). No
    // leading space either way: the counter ends the leader flush (T7).
    let counter = match kind.counter {
        SectionCounter::Count => format!("{total}"),
        SectionCounter::Progress => format!("{done}/{total}"),
    };
    let label = name.unwrap_or(kind.label).to_string();
    let spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled(kind.glyph, Style::default().fg(p.overlay1)),
        Span::styled(" ", Style::default()),
        Span::styled(
            label,
            Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default()),
    ];
    project_row_trailing(
        spans,
        Span::styled(counter, Style::default().fg(p.overlay0)),
        Some(('·', Style::default().fg(p.surface1))),
        width,
    )
}

/// Fourth-level Project-view row: a single COMMANDS/CHECKS entry — a state
/// bullet, its label, and an optional right-aligned detail (e.g. a port).
/// The bullet is kind-aware: COMMANDS marks running entries (`●`), CHECKS
/// rows exist only to flag failures (a provider error row included), so an
/// idle CHECKS row gets the red `✗` from the design mockup, not the dim dot.
pub(crate) fn section_item_line(
    kind: &'static SectionDescriptor,
    label: &str,
    detail: Option<&str>,
    running: bool,
    p: &Palette,
    width: u16,
) -> Line<'static> {
    // Bullet style is a declared field on the descriptor (`SectionBullet`)
    // now, not a wildcard match on the kind (bora-by6 G6).
    let (bullet, bullet_style) = match (kind.bullet, running) {
        (SectionBullet::FlagIdleAsError, false) => ("✗", Style::default().fg(p.red)),
        (_, true) => ("●", Style::default().fg(p.green)),
        (_, false) => ("·", Style::default().fg(p.overlay0)),
    };
    let mut spans = vec![
        Span::styled("   ", Style::default()),
        Span::styled(bullet, bullet_style),
        Span::styled(" ", Style::default()),
    ];
    let trailing = detail.map(|d| Span::styled(d.to_string(), Style::default().fg(p.overlay0)));
    let prefix_width: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    let trailing_width = trailing
        .as_ref()
        .map(|s| display_width(s.content.as_ref()))
        .unwrap_or(0);
    let avail = (width as usize).saturating_sub(prefix_width + trailing_width);
    spans.push(Span::styled(
        truncate_end(label, avail),
        Style::default().fg(p.overlay1),
    ));
    match trailing {
        Some(trailing) => project_row_trailing(spans, trailing, None, width),
        None => Line::from(spans),
    }
}

/// Pane order: `public_pane_numbers` sorted ascending — the same order the
/// old per-pane `PaneRow` path used to emit, so a workspace's pane sequence
/// reads the same as before this shape change. That responsibility now
/// lives ENTIRELY here: `project_view::push_pane_dots_row` carries no pane
/// data at all (`PaneDotsRow`'s own doc), so this is the single place "a
/// workspace's pane order" is defined for Project view. `number` rides
/// along so both callers derive the same `wNpN` address via
/// `project_view::pane_address` without a second lookup or a second
/// convention for the same string.
///
/// Indent shared by BOTH lines of a `PaneDotsRow` block (bora-79l F2,
/// ALVO_CAPTURE rows 04-05/28-29, `ui::sidebar::capture`): column 3 (3
/// leading cells), matching the section's own base indent (`SectionRow`'s
/// ` ⎇ ` glyph column) plus the two extra columns the approved mock gives
/// child rows. l1 (name) and l2 (dots) each start here independently now —
/// unlike the old single-line row, they no longer share one column budget,
/// so there is no separate name/dots gap constant to keep in lockstep with
/// the name's rendered width anymore.
const PANE_DOTS_INDENT: u16 = 3;

/// Ordered `(pane, public number, column)` for one workspace's l2 dots
/// line (bora-79l F2; replaces the old single-line row's `pane_dots_layout`,
/// which anchored dot columns to the rendered name on the SAME line — that
/// coupling no longer exists once the name moved to its own l1).
///
/// THIRD lockstep consumer alongside `render_workspace_list` and
/// `workspace_list_areas_for_entries` (see `entry_row_height`'s contract):
/// the renderer draws the dots at these columns and the geometry pass makes
/// each dot's hit area from them, so a disagreement here is a click that
/// focuses the wrong pane rather than a visible error.
///
/// One dot per pane, one column each, separated by a single space, starting
/// at `PANE_DOTS_INDENT` — never right-pinned to the row's right edge
/// (bora-c1h's "dots anchored to a shared, reproducible column" lesson:
/// the two passes must derive the SAME columns from the SAME inputs, which
/// `width` and the workspace's own pane count already are). Past the point
/// where even a single dot cannot fit, this returns fewer triples than the
/// workspace has panes rather than drawing off the row.
fn pane_dots_columns(
    ws: &crate::workspace::Workspace,
    width: u16,
) -> Vec<(crate::layout::PaneId, usize, u16)> {
    let mut panes: Vec<(crate::layout::PaneId, usize)> = ws
        .public_pane_numbers
        .iter()
        .map(|(&id, &number)| (id, number))
        .collect();
    panes.sort_by_key(|(_, number)| *number);

    // Largest `n` with `2n - 1 <= avail` (one cell per dot, one separating
    // space between each pair), 0 when even a single dot does not fit.
    let avail = width.saturating_sub(PANE_DOTS_INDENT);
    let max_dots = avail.div_ceil(2) as usize;
    panes.truncate(max_dots);

    panes
        .into_iter()
        .enumerate()
        .map(|(i, (pane_id, number))| (pane_id, number, PANE_DOTS_INDENT + (i * 2) as u16))
        .collect()
}

/// L1 of a Project-view workspace's `PaneDotsRow` block (bora-79l F2,
/// ALVO_CAPTURE rows 04/28): the workspace's own already-disambiguated
/// unique name, indented to `PANE_DOTS_INDENT` and colored `overlay1` — the
/// SAME color `⎇` uses on its `SectionRow` (gate G1). T7 (bora-79l,
/// divergence A) removed the `+N −M` diff this line carried since T2: no
/// PaneDotsRow l1 ever carries a diff — the numbers live ONLY in the
/// header's cluster (ALVO row 28 is bare `hotfix`, row 27 carries
/// `+916 −2`), so the diff-width reservation went with it. No state glyph
/// lands here either; every pane's state is l2's payload
/// (`pane_dots_dots_line`). Deliberately NOT dim in any pane state: the old
/// single-line row dimmed this name because it shared a row with the
/// (louder) dots; split onto its own line, it reads at the same weight the
/// branch glyph does — "a parte importante não some", inclusive parado.
fn pane_dots_name_line(name: &str, p: &Palette, width: u16) -> Line<'static> {
    let avail = width.saturating_sub(PANE_DOTS_INDENT) as usize;
    Line::from(vec![
        Span::styled(" ".repeat(PANE_DOTS_INDENT as usize), Style::default()),
        Span::styled(truncate_end(name, avail), Style::default().fg(p.overlay1)),
    ])
}

/// L2 of a Project-view workspace's `PaneDotsRow` block (bora-79l F2,
/// ALVO_CAPTURE rows 05/29): one state dot per pane, in the SAME order
/// `pane_dots_columns` returned (so glyph `i` in `dots` is drawn at
/// `pane_dots_columns`'s dot `i`), separated by a single space, starting at
/// `PANE_DOTS_INDENT`.
fn pane_dots_dots_line(dots: &[(&'static str, Style)], width: u16) -> Line<'static> {
    let mut spans = vec![Span::styled(
        " ".repeat(PANE_DOTS_INDENT as usize),
        Style::default(),
    )];
    let mut used = PANE_DOTS_INDENT;
    for (i, (glyph, style)) in dots.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default()));
            used = used.saturating_add(1);
        }
        spans.push(Span::styled(*glyph, *style));
        used = used.saturating_add(1);
    }
    // Pad to the full row so the row totals exactly `width`, matching every
    // other line builder here.
    if let Some(rest) = width.checked_sub(used) {
        if rest > 0 {
            spans.push(Span::styled(" ".repeat(rest as usize), Style::default()));
        }
    }
    Line::from(spans)
}

/// Live glyph + style for one `PaneDotsRow` l2 dot (bora-79l F2, "the glyph
/// convergence itself is F2's leaf" — `capture::alvo_fixture`'s doc). T2
/// (bora-79l) closes the convergence: the design's five "estados da
/// bolinha", one hue per meaning (R1), mapped onto `AgentState` + `seen` by
/// each state's own gloss in the anatomy's "Os estados da bolinha do
/// painel" block — with every ALVO_CAPTURE text row preserved byte for
/// byte (◆ stays ◆, ● stays ●; only hues/bold move):
///
/// - `Working` — "trabalhando" — the shared Braille spinner
///   (`super::spinner_frame`), overlay1+BOLD per the alvo mock's `.spin.o1.b`
///   (cinza, R1: the spinner is plumbing, not a state hue).
/// - `Blocked`+unseen — "esperando VOCÊ · agent parou pra perguntar" — ●
///   amarelo: `Blocked` is literally "needs human input and is blocked on a
///   response", and unseen means the question has not even been read yet.
/// - `Idle`+unseen — "respondeu / pronto · terminou, vem ler" — ● verde:
///   `Idle` is "agent finished" and unseen is "vem ler".
/// - `Blocked`+seen — "falhou" — the shared `blocked_glyph` (the repo's
///   falha glyph, dots/symbols preference) in red BOLD, exactly the state
///   `capture::alvo_fixture`'s "main" (Blocked+seen) pins as ALVO row 05's
///   `◆ falha real`.
/// - `Idle`+seen / `Unknown` — "parado" — hollow ○ overlay0, plain: a
///   finished-and-read agent and a plain shell both read as quiet.
///
/// The bullets are STATIC, never `state_dot`'s animated sand-timer: a
/// steady ● is never mistaken for ongoing work the way `Working`'s spinner
/// is. `Working`/falha reuse the SAME shared glyph sets `state_dot`/
/// `agent_icon` use elsewhere (`super::spinner_frame`, `blocked_glyph`)
/// rather than a second convention drifting from theirs.
fn pane_dots_dot_glyph(
    state: AgentState,
    seen: bool,
    tick: u32,
    indicator_style: crate::config::StatusIndicatorStyle,
    p: &Palette,
) -> (&'static str, Style) {
    match (state, seen) {
        (AgentState::Working, _) => (
            super::spinner_frame(tick),
            Style::default().fg(p.overlay1).add_modifier(Modifier::BOLD),
        ),
        (AgentState::Blocked, false) => (
            "●",
            Style::default().fg(p.yellow).add_modifier(Modifier::BOLD),
        ),
        (AgentState::Idle, false) => (
            "●",
            Style::default().fg(p.green).add_modifier(Modifier::BOLD),
        ),
        (AgentState::Blocked, true) => (
            blocked_glyph(indicator_style),
            Style::default().fg(p.red).add_modifier(Modifier::BOLD),
        ),
        (AgentState::Idle, true) => ("○", Style::default().fg(p.overlay0)),
        (AgentState::Unknown, _) => ("○", Style::default().fg(p.overlay0)),
    }
}

/// Sixth-level Project-view row: one PULL REQUESTS entry — PR number,
/// title, and a trailing checks-rollup glyph (bora-yw6.2). A draft PR dims
/// its number and gets a hollow bullet instead of the solid state dot.
pub(crate) fn pr_row_line(
    number: u64,
    title: &str,
    is_draft: bool,
    checks: Option<crate::workspace::ChecksRollup>,
    p: &Palette,
    width: u16,
) -> Line<'static> {
    let (bullet, bullet_style) = if is_draft {
        ("◌", Style::default().fg(p.subtext0))
    } else {
        ("●", Style::default().fg(p.green))
    };
    let number_style = if is_draft {
        Style::default().fg(p.subtext0)
    } else {
        Style::default().fg(p.overlay1)
    };
    let mut spans = vec![
        Span::styled("      ", Style::default()),
        Span::styled(bullet, bullet_style),
        Span::styled(" ", Style::default()),
        Span::styled(format!("#{number} "), number_style),
    ];
    let trailing = pr_checks_glyph(checks, p).map(|(glyph, style)| Span::styled(glyph, style));
    let prefix_width: usize = spans
        .iter()
        .map(|s| display_width(s.content.as_ref()))
        .sum();
    let trailing_width = trailing
        .as_ref()
        .map(|s| display_width(s.content.as_ref()))
        .unwrap_or(0);
    let avail = (width as usize).saturating_sub(prefix_width + trailing_width);
    spans.push(Span::styled(
        truncate_end(title, avail),
        Style::default().fg(p.overlay1),
    ));
    match trailing {
        Some(trailing) => project_row_trailing(spans, trailing, None, width),
        None => Line::from(spans),
    }
}

/// Glyph + style for A1's `OpenPr.checks: Option<ChecksRollup>` (bora-yw6.2,
/// contract C1) — A2 owns only this mapping; the precedence and
/// GitHub-conclusion-string mapping that PRODUCE the rollup are A1's
/// (`workspace::git::open_prs`). `None` means no checks reported for the
/// head commit — no trailing glyph, exactly like `checks_badge` returning
/// `None` for a PR with zero check runs. Delegates to `checks_rollup_glyph`
/// so the palette never drifts from the worktree CHECKS band's own glyphs.
fn pr_checks_glyph(
    checks: Option<crate::workspace::ChecksRollup>,
    p: &Palette,
) -> Option<(&'static str, Style)> {
    Some(checks_rollup_glyph(checks?, p))
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
    ws.channel_home_name().is_some().then_some(channel_group)
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
    if ws.channel_home_name().is_some() {
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
    if app.view_mode == crate::config::ViewMode::Project {
        // Three levels, built by the pure module. Never falls through to the
        // Flat or Repo paths below: the Repo view and its tests stay untouched.
        let entries = project_view::project_view_entries(app, force_expanded);
        if force_expanded {
            return entries;
        }
        return apply_hidden_filter(app, &std::collections::HashSet::new(), entries);
    }

    if !app.groups_workspaces() {
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
    emission_order.sort_by_key(|&idx| app.workspaces[idx].channel_home_name().is_none());

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
                            if member_ws.channel_home_name().is_some() {
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
        // Project view depths. The hidden filter drops a header whose members
        // all became hidden, so these must nest correctly or a project row
        // would survive with nothing under it.
        WorkspaceListEntry::ProjectRow { .. } => 0,
        WorkspaceListEntry::WorktreeRow { .. } | WorkspaceListEntry::SectionRow { .. } => 1,
        WorkspaceListEntry::SectionHeader { .. } => 2,
        WorkspaceListEntry::SectionItem { .. } => 3,
        WorkspaceListEntry::PrRow { .. } => 3,
        // Strictly deeper than `Workspace`, whose child it is.
        WorkspaceListEntry::PaneDotsRow { .. } => 4,
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
            // 6a: `PaneDotsRow` is the workspace child the hidden filter
            // counts — every member has exactly one block. A
            // `SectionRow` is the GROUP container now (its `ws_idx`
            // names only the representative), so it stopped being a
            // child and became a header below.
            WorkspaceListEntry::Workspace { ws_idx, .. }
            | WorkspaceListEntry::PaneDotsRow { ws_idx, .. } => {
                let hidden = ws_hidden(*ws_idx);
                for &h in &open {
                    had_child[h] = true;
                    has_kept_child[h] |= !hidden;
                }
            }
            WorkspaceListEntry::GroupHeader { .. }
            | WorkspaceListEntry::ProjectHeader { .. }
            | WorkspaceListEntry::BranchHeader { .. }
            | WorkspaceListEntry::ProjectRow { .. }
            | WorkspaceListEntry::WorktreeRow { .. }
            | WorkspaceListEntry::SectionRow { .. }
            | WorkspaceListEntry::SectionHeader { .. } => open.push(i),
            WorkspaceListEntry::HiddenHeader { .. }
            | WorkspaceListEntry::SectionItem { .. }
            | WorkspaceListEntry::PrRow { .. } => {}
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
            // Project view. A pane row belongs to a workspace, so it hides
            // with it; the container rows follow the same all-children-hidden
            // rule as their repo-view counterparts above. `PaneDotsRow`
            // (bora Project-view row-shape rework) replaced the old
            // per-pane `PaneRow` here — same rule, same `ws_idx` source of
            // truth.
            WorkspaceListEntry::PaneDotsRow { ws_idx, .. } => {
                if !ws_hidden(*ws_idx) {
                    result.push(entry);
                }
            }
            // 6a: the section row is the GROUP container — it drops when
            // every member block below it hid (same all-children-hidden
            // rule as the project row), never on its representative
            // alone. `wsec:` collapse keys never enter the hidden set,
            // so `is_hidden` stays false for an expanded group.
            WorkspaceListEntry::SectionRow { .. } => {
                if !had_child[i] || has_kept_child[i] {
                    result.push(entry);
                }
            }
            WorkspaceListEntry::ProjectRow { collapse_key, .. }
            | WorkspaceListEntry::WorktreeRow { collapse_key, .. } => {
                let drop = app.is_hidden(collapse_key) || (had_child[i] && !has_kept_child[i]);
                if !drop {
                    result.push(entry);
                }
            }
            WorkspaceListEntry::SectionHeader { .. }
            | WorkspaceListEntry::SectionItem { .. }
            | WorkspaceListEntry::PrRow { .. } => result.push(entry),
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

pub(crate) fn workspace_list_body_rect(_app: &AppState, area: Rect, has_scrollbar: bool) -> Rect {
    if area.width == 0 || area.height <= WORKSPACE_LIST_TOP_MARGIN_ROWS + 1 {
        return Rect::default();
    }

    let body_y = area.y.saturating_add(WORKSPACE_LIST_TOP_MARGIN_ROWS);
    let footer_y = (area.y + area.height).saturating_sub(1);
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
        let needed = entry_row_height(entry, &entries, entry_idx, app.sidebar_project.row_gap);
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

/// Core geometry walk shared by `compute_workspace_list_areas` (Flat/Repo
/// card + group-header areas) and `compute_project_row_areas` (Project-view
/// row areas). Takes entries and the body rect directly rather than
/// deriving them from `app` itself, so every arm except `PaneDotsRow` stays
/// testable with hand-built entries and `AppState::test_new()` — no
/// dependency on the entries builder in `sidebar::project_view`.
///
/// `app` exists for `PaneDotsRow` and `SectionRow`: the dots' per-dot hit
/// areas need each pane's live `pane_id`, which (unlike every other row
/// here) is not carried on the entry (`PaneDotsRow`'s doc comment explains
/// why), and the header's `SectionNew` "+" target (T4, bora-79l) carries
/// the section's `(repo_identity, branch)` read from the live workspace at
/// walk time — the same render-time-read rule the `SectionRow` render arm
/// already states for git/PR state, kept off the entry so a stale branch
/// can never outlive one frame.
fn workspace_list_areas_for_entries(
    entries: &[WorkspaceListEntry],
    app: &AppState,
    scroll: usize,
    body: Rect,
    row_gap: u16,
) -> (
    Vec<crate::app::state::WorkspaceCardArea>,
    Vec<crate::app::state::GroupHeaderCardArea>,
    Vec<ProjectRowHitArea>,
) {
    let mut row_y = body.y;
    let body_bottom = body.y + body.height;
    let mut cards = Vec::new();
    let mut headers: Vec<crate::app::state::GroupHeaderCardArea> = Vec::new();
    let mut project_rows: Vec<ProjectRowHitArea> = Vec::new();

    for (entry_idx, entry) in entries.iter().enumerate().skip(scroll) {
        let needed = entry_row_height(entry, entries, entry_idx, row_gap);
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
            WorkspaceListEntry::ProjectRow { collapse_key, .. } => {
                project_rows.push(ProjectRowHitArea {
                    rect: Rect::new(body.x, row_y, body.width, 1),
                    target: ProjectRowTarget::Project {
                        collapse_key: collapse_key.clone(),
                    },
                });
            }
            WorkspaceListEntry::WorktreeRow { checkout_key, .. } => {
                // WorktreeRow exists only for on-disk worktrees with no
                // open workspace now (bora-qdi) — every live checkout
                // renders as a `SectionRow` per workspace instead
                // (bora-c1h). A click always opens it.
                project_rows.push(ProjectRowHitArea {
                    rect: Rect::new(body.x, row_y, body.width, 1),
                    target: ProjectRowTarget::OpenWorktree {
                        checkout_key: checkout_key.clone(),
                    },
                });
            }
            WorkspaceListEntry::SectionRow {
                ws_idx,
                checkout_key,
                collapse_key,
                header_on,
                header_hidden,
                ..
            } => {
                // T3: a hidden header (model OFF, or the same-branch
                // exception) paints no line, so it claims no hit area —
                // an invisible affordance reads as a dead click. The row
                // itself still advances `row_y` via `entry_row_height`,
                // keeping this pass in lockstep with the renderer.
                if *header_on && !*header_hidden {
                    // T4 (bora-79l, P3): the header's trailing 3-cell "+"
                    // (create worktree in THIS section's context) — the
                    // same affordance the Flat/Repo repo headers carry
                    // (`worktree_new_hit_areas_from_headers`). It rides
                    // `project_rows` as its own target rather than the
                    // shared `worktree_new_hit_areas` vec because the
                    // dispatcher resolves `project_row_areas` FIRST, so a
                    // full-row `Section` area would otherwise swallow the
                    // click before the shared vec is ever consulted.
                    // Pushed BEFORE that area: `project_row_target_at`
                    // takes the first match, so inside the + cells the +
                    // wins and everywhere else on the row the Section
                    // behavior (collapse via caret, press suppression)
                    // is exactly as before — the same precedence the
                    // PaneDotsRow dot cells use against the block card.
                    // The target keys on the section's (repo, branch) —
                    // the branch_group pair, never `ws_idx` — so T6's
                    // same-branch section merge re-keys nothing. No
                    // emission without git identity + branch: the row
                    // renders, but there is no repo to create in.
                    if body.width >= 3 {
                        if let Some((repo_identity, branch)) =
                            app.workspaces.get(*ws_idx).and_then(|ws| {
                                Some((ws.git_space()?.repo_identity.clone(), ws.branch()?))
                            })
                        {
                            project_rows.push(ProjectRowHitArea {
                                rect: Rect::new(body.x + body.width - 3, row_y, 3, 1),
                                target: ProjectRowTarget::SectionNew {
                                    repo_identity,
                                    branch,
                                },
                            });
                        }
                    }
                    project_rows.push(ProjectRowHitArea {
                        rect: Rect::new(body.x, row_y, body.width, 1),
                        target: ProjectRowTarget::Section {
                            ws_idx: *ws_idx,
                            checkout_key: checkout_key.clone(),
                            collapse_key: collapse_key.clone(),
                        },
                    });
                }
                // No `WorkspaceCardArea` here (P2, bora-79l T1): the branch
                // line is not the workspace's representation — the
                // `PaneDotsRow` block right below carries the card now, and
                // with it every workspace-scoped affordance (click-to-switch,
                // right-click menu, press/drag, selection fill).
            }
            WorkspaceListEntry::SectionHeader { collapse_key, .. } => {
                project_rows.push(ProjectRowHitArea {
                    rect: Rect::new(body.x, row_y, body.width, 1),
                    target: ProjectRowTarget::Band {
                        collapse_key: collapse_key.clone(),
                    },
                });
            }
            WorkspaceListEntry::SectionItem {
                kind,
                label,
                ws_idx,
                ..
            } => {
                project_rows.push(ProjectRowHitArea {
                    rect: Rect::new(body.x, row_y, body.width, 1),
                    target: ProjectRowTarget::SectionItem {
                        kind,
                        label: label.clone(),
                        ws_idx: *ws_idx,
                    },
                });
            }
            // The block IS the workspace's card (P2, bora-79l T1): ONE
            // `WorkspaceCardArea` spanning BOTH lines (l1 name + l2 dots),
            // full body width — every workspace-scoped affordance
            // (click-to-switch, right-click menu, press/drag-reorder,
            // selection fill) keys off `cards`, so they all moved here
            // together from the `SectionRow` above. The dots stay
            // first-class: each keeps its own 1-cell `ProjectRowHitArea`
            // at the SAME column `render_workspace_list`'s `PaneDotsRow`
            // arm draws it — both call `pane_dots_columns`, so they cannot
            // drift — and the input dispatcher resolves `project_row_areas`
            // BEFORE `workspace_card_areas`, so inside a dot's own cell the
            // dot wins over the block.
            WorkspaceListEntry::PaneDotsRow { ws_idx, dots, .. } => {
                let card_height = if *dots { 2 } else { 1 };
                cards.push(crate::app::state::WorkspaceCardArea {
                    ws_idx: *ws_idx,
                    rect: Rect::new(body.x, row_y, body.width, card_height),
                    indented: true,
                });
                // T6 6b: no l2 row, no per-dot hit area — there is
                // nothing at `row_y + 1` for a dot to be hit-tested
                // against.
                if *dots {
                    if let Some(ws) = app.workspaces.get(*ws_idx) {
                        let dots_row_y = row_y.saturating_add(1);
                        for (_pane_id, number, column) in pane_dots_columns(ws, body.width) {
                            project_rows.push(ProjectRowHitArea {
                                rect: Rect::new(body.x + column, dots_row_y, 1, 1),
                                target: ProjectRowTarget::Pane {
                                    ws_idx: *ws_idx,
                                    pane_id: project_view::pane_address(ws, number),
                                },
                            });
                        }
                    }
                }
            }
            WorkspaceListEntry::PrRow { number, ws_idx, .. } => {
                // A row whose repo has no open workspace carries no `ws_idx`
                // and gets no hit area: there is nothing to name as the
                // worktree's repo, and guessing (e.g. the active workspace,
                // which is what the right panel's menu does) would create the
                // worktree in whatever repo happened to be focused. The row
                // still advances `row_y` below via `entry_row_height`, so the
                // three lockstep passes stay in agreement either way.
                if let Some(ws_idx) = ws_idx {
                    project_rows.push(ProjectRowHitArea {
                        rect: Rect::new(body.x, row_y, body.width, 1),
                        target: ProjectRowTarget::OpenPr {
                            ws_idx: *ws_idx,
                            number: *number,
                        },
                    });
                }
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

    (cards, headers, project_rows)
}

/// The whole geometry pass, once: workspace cards, group headers, and
/// Project-view row hit areas. `render`/`compute_view` MUST use this rather
/// than calling the two narrower wrappers below in sequence — each of those
/// rebuilds the entry list and re-walks every row, and this runs per render,
/// per pane, per attached client (AGENTS.md, "Multiplicative performance
/// paths").
pub(crate) fn compute_workspace_list_areas_all(
    app: &AppState,
    area: Rect,
) -> (
    Vec<crate::app::state::WorkspaceCardArea>,
    Vec<crate::app::state::GroupHeaderCardArea>,
    Vec<ProjectRowHitArea>,
) {
    let ws_area = workspace_list_rect(area, app.sidebar_section_split);
    if ws_area == Rect::default() {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let metrics = workspace_list_scroll_metrics(app, ws_area);
    let body = workspace_list_body_rect(app, ws_area, should_show_scrollbar(metrics));
    if body.width == 0 || body.height == 0 {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let entries = workspace_list_entries(app);
    workspace_list_areas_for_entries(
        &entries,
        app,
        app.workspace_scroll,
        body,
        app.sidebar_project.row_gap,
    )
}

pub(crate) fn compute_workspace_list_areas(
    app: &AppState,
    area: Rect,
) -> (
    Vec<crate::app::state::WorkspaceCardArea>,
    Vec<crate::app::state::GroupHeaderCardArea>,
) {
    let (cards, headers, _project_rows) = compute_workspace_list_areas_all(app, area);
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
        let selection_bg = workspace_selection_background(p, is_active);
        let row_style = if is_selected {
            Style::default().bg(selection_bg)
        } else if is_active {
            Style::default().bg(p.active_row_bg)
        } else {
            Style::default()
        };
        let num_style = if is_selected {
            Style::default().fg(p.overlay1).bg(selection_bg)
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
    render_agent_detail(app, terminal_runtimes, frame, detail_area);
    render_sidebar_toggle(app, frame, area, false, p);
}

/// Navigate-mode cursor background for a workspace row. Ported from upstream
/// herdr (#2987): when a theme leaves `selection_bg` unset, painting the
/// cursor row with it erases the active row's own fill, so the active Space
/// vanishes the moment the cursor lands on it. Fall back to `active_row_bg`
/// in exactly that case.
fn workspace_selection_background(p: &Palette, is_active: bool) -> Color {
    if is_active && p.selection_bg == Color::Reset {
        p.active_row_bg
    } else {
        p.selection_bg
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
    // Right-aligned view-mode toggle on the blank top-margin row (bora
    // regression fix: restores the toggle commit 7bb8133b dropped along
    // with the ` spaces` title it meant to remove — the title stays gone,
    // this alone comes back). Shares its row with the drag-reorder
    // "drop above the first card" indicator (`insertion_row` above): the
    // toggle only claims the row's trailing `label.len()` cells, so an
    // active drag's indicator line and this label can coexist, though the
    // indicator may run underneath the label's cells during a drag. That
    // overlap is accepted, not a bug to "fix" by adding a row back — see
    // `WORKSPACE_LIST_TOP_MARGIN_ROWS`'s own doc comment.
    if area.height > 0 {
        let toggle_rect = view_mode_toggle_rect(area, app.view_mode);
        if toggle_rect != Rect::default() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    app.view_mode.as_str(),
                    Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Right),
                toggle_rect,
            );
        }
    }
    let metrics = workspace_list_scroll_metrics(app, area);
    let scrollbar_rect = workspace_list_scrollbar_rect(app, area);

    // --- Render entries using the same lockstep iteration ---
    let entries = workspace_list_entries(app);
    let scroll = app.workspace_scroll;
    let body = workspace_list_body_rect(app, area, scrollbar_rect.is_some());
    let mut row_y = body.y;
    let now = Instant::now();
    // Branch label the most recently visited header row printed (None = it
    // printed none). Indented child rows consult this so they never repeat
    // a branch the header directly above them already shows.
    let mut parent_branch: Option<String> = None;
    // (T7, divergence A: the `parts.diff` ride-along this loop tracked for
    // l1's diff slot died with the slot — the flag now gates only the
    // `SectionRow` header cluster, read directly in that arm.)

    for (entry_idx, entry) in entries.iter().enumerate().skip(scroll) {
        let needed = entry_row_height(entry, &entries, entry_idx, app.sidebar_project.row_gap);
        if row_y.saturating_add(needed) > body.y + body.height {
            break;
        }
        match entry {
            WorkspaceListEntry::GroupHeader { name, collapse_key } => {
                parent_branch = None;
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
                parent_branch = branch.as_ref().map(|b| b.label.clone());
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
                    // The branch label span is inserted just before render
                    // (below) so it can be truncated to the width that
                    // actually remains after the row's chrome.
                    let mut branch_label_idx = None;
                    if let Some(b) = branch {
                        spans.push(Span::styled(" ", Style::default()));
                        branch_label_idx = Some(spans.len());
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
                    if let (Some(idx), Some(b)) = (branch_label_idx, branch.as_ref()) {
                        // Truncate the branch label to the width left after
                        // the row's fixed chrome (rail, name, ahead/behind,
                        // PR badge, collapse dot) — the same display_width
                        // budget the Workspace arm uses for workspace names.
                        // The +2 is the `[` `]` around the label.
                        let used: usize = spans
                            .iter()
                            .map(|s| display_width(s.content.as_ref()))
                            .sum();
                        let avail = (body.width as usize).saturating_sub(used + 2);
                        spans.insert(
                            idx,
                            Span::styled(
                                format!("[{}]", truncate_end(&b.label, avail)),
                                Style::default().fg(p.overlay1),
                            ),
                        );
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
                parent_branch = Some(label.clone());
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
                                workspace_selection_background(p, Some(idx) == app.active)
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
                    // The label span is inserted just before render (below)
                    // so it can be truncated to the width that remains.
                    let label_span_idx = spans.len();
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
                    // Truncate the branch label to the width left after the
                    // row's fixed chrome (connector, tab dots, ahead/behind,
                    // idle age or PR badge) — the same display_width budget
                    // the Workspace arm uses for workspace names.
                    let used: usize = spans
                        .iter()
                        .map(|s| display_width(s.content.as_ref()))
                        .sum();
                    let avail = (body.width as usize).saturating_sub(used);
                    spans.insert(
                        label_span_idx,
                        Span::styled(truncate_end(label, avail), name_style),
                    );
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::HiddenHeader { count } => {
                parent_branch = None;
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
            WorkspaceListEntry::ProjectRow {
                name,
                live,
                total,
                collapse_key,
                ..
            } => {
                if row_y < list_bottom {
                    // Slightly-lighter-than-background row fill (owner's
                    // ask, item 3c): `p.surface0` is the smallest lightness
                    // step up from `sidebar_bg` (which every built-in theme
                    // sets to `Color::Reset`, i.e. whatever the terminal's
                    // own background is — roughly `panel_bg` by design
                    // intent) that isn't already claimed by another row
                    // state in this file: `p.surface1` is the drag-preview
                    // fill (`Workspace`/`BranchHeader`-fold arms above) and
                    // `p.active_row_bg`/`p.selection_bg` are the
                    // active/cursor fills — reusing any of those here would
                    // make a plain project header look like one of those
                    // states. This background also now carries the visual
                    // weight `project_row_line`'s name span dropped BOLD
                    // for (item 6).
                    let buf = frame.buffer_mut();
                    for x in body.x..body.x + body.width {
                        buf[(x, row_y)].set_style(Style::default().bg(p.surface0));
                    }
                    let collapsed = app.collapsed_space_keys.contains(collapse_key);
                    frame.render_widget(
                        Paragraph::new(project_row_line(
                            name, *live, *total, collapsed, p, body.width,
                        )),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::WorktreeRow {
                repo,
                branch,
                ahead,
                behind,
                pr,
                collapse_key,
                unopened,
                ..
            } => {
                if row_y < list_bottom {
                    let collapsed = app.collapsed_space_keys.contains(collapse_key);
                    frame.render_widget(
                        Paragraph::new(worktree_row_line(
                            repo.as_deref(),
                            branch,
                            *ahead,
                            *behind,
                            *pr,
                            collapsed,
                            *unopened,
                            p,
                            body.width,
                        )),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::SectionHeader {
                kind,
                done,
                total,
                name,
                ..
            } => {
                if row_y < list_bottom {
                    frame.render_widget(
                        Paragraph::new(section_header_line(
                            kind,
                            *done,
                            *total,
                            name.as_deref(),
                            p,
                            body.width,
                        )),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::SectionItem {
                kind,
                label,
                detail,
                running,
                ..
            } => {
                if row_y < list_bottom {
                    frame.render_widget(
                        Paragraph::new(section_item_line(
                            kind,
                            label,
                            detail.as_deref(),
                            *running,
                            p,
                            body.width,
                        )),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::SectionRow {
                ws_idx,
                header_on,
                header_hidden,
                show_diff,
                diff,
                ..
            } => {
                // (T7, divergence A: this arm no longer tracks the
                // `parts.diff` switch for the paired `PaneDotsRow` — l1
                // lost its diff slot; the flag is read below for the
                // header's own cluster only.)
                // T3 (bora-79l): the header line renders only when the
                // model's switch is ON and the same-branch exception has
                if *header_on && !*header_hidden && row_y < list_bottom {
                    if let Some(ws) = app.workspaces.get(*ws_idx) {
                        let is_worktree = ws
                            .worktree_space()
                            .is_some_and(|space| space.is_linked_worktree);
                        let branch = ws.branch();
                        let (ahead, behind) = ws.git_ahead_behind().unwrap_or((0, 0));
                        // 6a: the `+N −M` slot is the GROUP's summed
                        // change set (folded at emission over every
                        // member, `SectionRow::diff`'s doc), gated by the
                        // section model's `parts.diff` — never a single
                        // member's numbers wearing the group's header.
                        let diff = if *show_diff { *diff } else { None };
                        let pr = ws
                            .cached_check_status
                            .as_ref()
                            .and_then(|status| status.pr.as_ref())
                            .map(|pr| {
                                let tone = match pr.state.to_ascii_uppercase().as_str() {
                                    "MERGED" => PrChipTone::Merged,
                                    "DRAFT" => PrChipTone::Draft,
                                    "CLOSED" => PrChipTone::Closed,
                                    _ => PrChipTone::Open,
                                };
                                (pr.number, tone)
                            });
                        let checks = ws
                            .cached_check_status
                            .as_ref()
                            .and_then(|status| crate::workspace::checks_rollup(&status.checks));
                        let glyphs = crate::config::project_glyphs(app.sidebar_project.glyph_style);
                        // T4 (bora-79l, P3): the header's "+" overlay —
                        // the Flat/Repo affordance convention (3 cells,
                        // trailing edge, overlay1, `mouse_capture`-gated).
                        // Unlike the Flat/Repo headers, whose content
                        // rarely reaches the edge, this row's state
                        // cluster pins FLUSH right (T7 divergence B), so
                        // the cluster's width budget is shrunk by the
                        // reserve instead of letting the glyph overwrite
                        // cluster cells — `section_row_line`'s own rule
                        // ("the cluster never loses a cell") extended to
                        // the +. Painted only when the hit area would be
                        // emitted (git identity + branch resolve): a
                        // glyph with no action is a dead affordance.
                        let plus_w = if app.mouse_capture
                            && body.width >= 3
                            && ws.git_space().is_some()
                            && ws.branch().is_some()
                        {
                            3
                        } else {
                            0
                        };
                        frame.render_widget(
                            Paragraph::new(section_row_line(
                                is_worktree,
                                branch.as_deref(),
                                diff,
                                ahead,
                                behind,
                                pr,
                                checks,
                                &glyphs,
                                p,
                                body.width - plus_w,
                            )),
                            Rect::new(body.x, row_y, body.width, 1),
                        );
                        if plus_w > 0 {
                            frame.render_widget(
                                Paragraph::new(" + ").style(Style::default().fg(p.overlay1)),
                                Rect::new(body.x + body.width - 3, row_y, 3, 1),
                            );
                        }
                    }
                }
            }
            WorkspaceListEntry::PaneDotsRow {
                ws_idx,
                name,
                dots: dots_on,
            } => {
                // The block carries the workspace's visual state now (P2,
                // bora-79l T1) — the same GC3 three-way decision the
                // Flat/Repo `Workspace` card uses: a real selection
                // (navigate cursor or in-flight drag) fills BOTH rows so
                // the block changes colour as one piece, while the plain
                // "this is the active workspace" case is a lighter
                // statement — the blue bar at the block's left border —
                // so the dots' own state colours stay legible on the
                // active row too. T6 6b: `block_height` is the single
                // source both the selection fill and the active-bar loop
                // below read, so they can never desync from
                // `entry_row_height` when `dots` is off (1 row, not 2).
                let block_height: u16 = if *dots_on { 2 } else { 1 };
                let selected = *ws_idx == app.selected && is_navigating;
                let is_active = Some(*ws_idx) == app.active;
                let is_dragged = dragged_ws_idx == Some(*ws_idx);
                let selection_paint = selected || is_dragged;
                let show_active_marker = is_active && !selection_paint;
                if selection_paint {
                    let bg = if selected {
                        workspace_selection_background(p, is_active)
                    } else {
                        p.surface1
                    };
                    let buf = frame.buffer_mut();
                    for y in row_y..row_y.saturating_add(block_height) {
                        if y >= list_bottom {
                            break;
                        }
                        for x in body.x..body.x + body.width {
                            buf[(x, y)].set_style(Style::default().bg(bg));
                        }
                    }
                }
                if let Some(ws) = app.workspaces.get(*ws_idx) {
                    // L1: the workspace name (bora-79l F2 —
                    // `pane_dots_name_line`'s doc, ALVO_CAPTURE rows 04/28;
                    // T7 divergence A removed the diff slot this carried).
                    // No state glyph ever lands here.
                    if row_y < list_bottom {
                        frame.render_widget(
                            Paragraph::new(pane_dots_name_line(name, p, body.width)),
                            Rect::new(body.x, row_y, body.width, 1),
                        );
                        if show_active_marker {
                            // The blue bar (contract T1 item 3), painted
                            // AFTER the line so it lands on the indent's
                            // blank first cell — `PANE_DOTS_INDENT` keeps
                            // every glyph clear of column 0.
                            let buf = frame.buffer_mut();
                            buf[(body.x, row_y)].set_symbol("▎");
                            buf[(body.x, row_y)].set_style(Style::default().fg(p.accent));
                        }
                    }
                    // L2: one dot per pane. Live state per dot, resolved
                    // here rather than carried on the entry (`PaneDotsRow`'s
                    // doc) — one `pane_details` call for the whole row, not
                    // one per dot, so this stays cheaper than the old
                    // per-`PaneRow` render path it replaces (which called
                    // `pane_details` once PER PANE). T6 6b: skipped
                    // entirely when `dots` is off — there is no l2 row.
                    if *dots_on {
                        let dots_row_y = row_y.saturating_add(1);
                        if dots_row_y < list_bottom {
                            let columns = pane_dots_columns(ws, body.width);
                            let details = ws.pane_details(&app.terminals);
                            let dots: Vec<(&'static str, Style)> = columns
                                .iter()
                                .map(|(pane_id, _number, _column)| {
                                    details
                                        .iter()
                                        .find(|d| d.pane_id == *pane_id)
                                        .map(|d| {
                                            pane_dots_dot_glyph(
                                                d.state,
                                                d.seen,
                                                app.spinner_tick,
                                                app.status_indicators,
                                                p,
                                            )
                                        })
                                        .unwrap_or(("○", Style::default().fg(p.overlay0)))
                                })
                                .collect();
                            frame.render_widget(
                                Paragraph::new(pane_dots_dots_line(&dots, body.width)),
                                Rect::new(body.x, dots_row_y, body.width, 1),
                            );
                            if show_active_marker {
                                let buf = frame.buffer_mut();
                                buf[(body.x, dots_row_y)].set_symbol("▎");
                                buf[(body.x, dots_row_y)].set_style(Style::default().fg(p.accent));
                            }
                        }
                    }
                }
            }
            WorkspaceListEntry::PrRow {
                number,
                title,
                is_draft,
                checks,
                ..
            } => {
                if row_y < list_bottom {
                    frame.render_widget(
                        Paragraph::new(pr_row_line(
                            *number, title, *is_draft, *checks, p, body.width,
                        )),
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
                // GC3: only a real "selection" (keyboard cursor or an
                // in-flight drag) repaints the whole row — a background
                // tint reads as *selected*. The plain "this is the active
                // workspace" case is a lighter statement and gets a
                // left-edge marker instead (`show_active_marker` below),
                // so its own state colour (GC1) stays fully legible on the
                // active row too, instead of being overridden by it.
                let selection_paint = selected || is_dragged;
                let show_active_marker = is_active && !selection_paint;

                // Card rect spans 1 row (name + inline dots).
                let card_height = 1u16;
                if selection_paint {
                    let bg = if selected {
                        workspace_selection_background(p, is_active)
                    } else {
                        p.surface1
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

                // GC1: an inactive row's text colour follows its own status
                // dot and its weight follows how urgently it wants the
                // user — ranked through the single `attention_priority`
                // owner, never a local table, mirroring
                // `render_agent_detail`'s `wants_attention` (the same
                // pattern already proven on the agent panel, applied here
                // to the row it was missing from).
                let (row_state, row_seen) = ws.aggregate_state(&app.terminals);
                let idle_age = ws
                    .oldest_unseen_idle_age(&app.terminals, now)
                    .or_else(|| ws.oldest_idle_age(&app.terminals, now));
                let (_, dot_style) = state_dot(
                    row_state,
                    row_seen,
                    app.spinner_tick,
                    app.status_indicators,
                    p,
                    idle_age,
                );
                let dot_color = dot_style.fg.unwrap_or(p.subtext0);
                let attn = crate::detect::attention_priority(row_state, row_seen);
                let working_attn =
                    crate::detect::attention_priority(crate::detect::AgentState::Working, true);
                let state_style = if attn > working_attn {
                    Style::default().fg(dot_color).add_modifier(Modifier::BOLD)
                } else if attn == working_attn {
                    Style::default().fg(dot_color)
                } else {
                    Style::default().fg(dot_color).add_modifier(Modifier::DIM)
                };
                let name_style = if selection_paint {
                    Style::default().fg(p.text).add_modifier(Modifier::BOLD)
                } else {
                    state_style
                };
                let rail_style = Style::default().fg(p.overlay0);

                // --- Single row: name + inline tab dots ---
                let mut line1 = Vec::new();
                // GC3: a reserved 1-column lane, blank on every row but the
                // active one, so the marker's presence never shifts
                // anything but itself — columns line up whether or not a
                // given row is active.
                if show_active_marker {
                    line1.push(Span::styled("▎", Style::default().fg(p.accent)));
                } else {
                    line1.push(Span::styled(" ", Style::default()));
                }
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
                // Idle time (hoisted above, GC1) follows the same age
                // color ramp whether the idle pane was already seen or not.
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

                // An indented child drops the repo-derived name (not unique
                // under its header — same checkout+branch siblings collide):
                // the row is its `@wNpN` badge plus, only when the header
                // above did not print it, the branch. The badge is computed
                // once here and reused for the row's ` @name` suffix; a
                // plain-shell pane reports no agent identity at all, and
                // such a row keeps the display name — a duplicate name still
                // says more than an anonymous empty label.
                let agent_badge = workspace_agent_label(ws, &app.terminals);
                let full_label = if *indented && agent_badge.is_some() {
                    indented_child_label(ws, parent_branch.as_deref())
                } else {
                    ws.display_name_from(&app.terminals, terminal_runtimes)
                };
                let token_spans: Vec<Span<'static>> = if ws.metadata_tokens.is_empty() {
                    Vec::new()
                } else {
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
                let agent_suffix = agent_badge.map(|name| format!(" @{name}"));
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
        // DIM is how this list says "background". A row that wants the user is
        // precisely what must not read as background, so an inactive Blocked or
        // finished-but-unseen agent keeps its full-strength label colour. Ranked
        // against `Working` through the single owner in `crate::detect` rather
        // than by listing states here, so this cannot drift away from the
        // ordering every other surface uses.
        let wants_attention = crate::detect::attention_priority(detail.state, detail.seen)
            > crate::detect::attention_priority(crate::detect::AgentState::Working, true);
        let status_style = if is_active || wants_attention {
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
    use super::project_view::{CHECKS, COMMANDS, NOTES, PULL_REQUESTS, TODOS};
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

        // Detected only: falls back to the pane's addressable id, never the
        // agent kind — "pi"/"omp" names a tool, not an agent.
        let ws = &app.workspaces[0];
        let expected_addr = format!(
            "{}p{}",
            ws.id,
            crate::workspace::encode_public_number(ws.public_pane_number(first_pane).unwrap())
        );
        let detected_only = workspace_agent_label(ws, &app.terminals);
        assert_eq!(detected_only.as_deref(), Some(expected_addr.as_str()));

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
        app.view_mode = crate::config::ViewMode::Flat;
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

        // The panel is retired from the live sidebar layout (bora-49p.6) but
        // its rendering is retained, so paint it directly into an area instead
        // of asking the layout for one it no longer allots. What this test
        // guards is the row content, which is unchanged either way.
        let agent_area = Rect::new(0, 0, 25, 10);
        let mut terminal = Terminal::new(TestBackend::new(25, 10)).unwrap();
        terminal
            .draw(|frame| {
                render_agent_detail(&app, &TerminalRuntimeRegistry::new(), frame, agent_area)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
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

        // Panel retired from the live layout (bora-49p.6); its rendering is
        // retained, so paint it directly into an area of our own.
        let agent_area = Rect::new(0, 0, 25, 10);
        let mut terminal = Terminal::new(TestBackend::new(25, 10)).unwrap();
        terminal
            .draw(|frame| {
                render_agent_detail(&app, &TerminalRuntimeRegistry::new(), frame, agent_area)
            })
            .unwrap();
        let body = agent_panel_body_rect(agent_area, false);
        let buffer = terminal.backend().buffer();
        let workspace = buffer[(find_symbol_x(buffer, body.y, body.width, "o"), body.y)].style();
        let agent = buffer[(find_symbol_x(buffer, body.y, body.width, "p"), body.y)].style();

        assert_eq!(workspace.fg, Some(app.palette.text));
        assert!(!workspace.add_modifier.contains(Modifier::BOLD));
        assert_eq!(agent.fg, Some(app.palette.overlay0));
        assert!(!agent.add_modifier.contains(Modifier::DIM));
    }

    /// Renders the agent panel with one pane in `state` and NOTHING active, then
    /// returns the style of that row's state label.
    fn inactive_state_label_style(state: AgentState, seen: bool) -> ratatui::style::Style {
        let config: crate::config::Config = toml::from_str(
            r##"
[ui.sidebar.agents]
rows = [[{ token = "state_text" }]]
"##,
        )
        .expect("fixture config parses");
        let mut app = crate::app::state::AppState::test_new();
        app.sidebar_agents = config.ui.sidebar.agents;
        let workspace = Workspace::test_new("one");
        let pane_id = workspace.tabs[0].root_pane;
        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        // `is_active_pane` returns false for every pane when nothing is active,
        // which is the branch under test.
        app.active = None;
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app
            .terminals
            .get_mut(&terminal_id)
            .expect("ensure_test_terminals created this terminal");
        terminal.state = state;
        terminal.detected_agent = Some(Agent::Pi);
        app.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane_id)
            .expect("root pane exists")
            .seen = seen;

        let area = Rect::new(0, 0, 40, 10);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).expect("test backend");
        terminal
            .draw(|frame| render_agent_detail(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .expect("draw succeeds");
        let body = agent_panel_body_rect(area, false);
        let buffer = terminal.backend().buffer();
        let label = crate::ui::status::state_label(state, seen);
        let text = row_text(buffer, body.y, body.width);
        let x = text
            .find(label)
            .unwrap_or_else(|| panic!("label {label:?} missing from row {text:?}"));
        buffer[(u16::try_from(x).expect("label offset fits a u16"), body.y)].style()
    }

    /// DIM is how this panel says "background", so a row that wants the user must
    /// not be dimmed merely for not being the active one. Before this, `DIM` was
    /// applied to every inactive row's label unconditionally, which muted the red
    /// `blocked` label — defeating the one thing the sidebar most needs to say.
    ///
    /// Two-sided on purpose: `working` and `idle` are still dimmed, so this fails
    /// both if the DIM comes back for attention rows and if it is dropped
    /// wholesale.
    #[test]
    fn inactive_rows_are_dimmed_except_the_ones_that_want_you() {
        let blocked = inactive_state_label_style(AgentState::Blocked, true);
        assert!(
            !blocked.add_modifier.contains(Modifier::DIM),
            "a blocked agent's label must not be dimmed just because its row is inactive"
        );

        let done = inactive_state_label_style(AgentState::Idle, false);
        assert!(
            !done.add_modifier.contains(Modifier::DIM),
            "an agent that finished while you were away still wants you"
        );

        let working = inactive_state_label_style(AgentState::Working, true);
        assert!(
            working.add_modifier.contains(Modifier::DIM),
            "a working agent does not want you, so its inactive row stays dimmed"
        );

        let idle = inactive_state_label_style(AgentState::Idle, true);
        assert!(
            idle.add_modifier.contains(Modifier::DIM),
            "a seen-idle agent does not want you either"
        );
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

        // GC1: neither workspace has a detected agent, so both are the
        // "Unknown" state and get the SAME dim overlay0 name style — a
        // row's colour and weight now follow its own state, not "is this
        // the active workspace" (that used to force `text`+BOLD here).
        let active = buffer[(find_symbol_x(buffer, first_row, 25, "o"), first_row)].style();
        assert_eq!(active.fg, Some(app.palette.overlay0));
        assert!(active.add_modifier.contains(Modifier::DIM));
        assert!(!active.add_modifier.contains(Modifier::BOLD));
        assert_eq!(active.bg, Some(ratatui::style::Color::Reset));

        let inactive = buffer[(find_symbol_x(buffer, second_row, 25, "t"), second_row)].style();
        assert_eq!(inactive.fg, Some(app.palette.overlay0));
        assert!(inactive.add_modifier.contains(Modifier::DIM));
        assert!(!inactive.add_modifier.contains(Modifier::BOLD));
        assert_eq!(inactive.bg, Some(ratatui::style::Color::Reset));

        // GC3: the active row is still marked, but by the left-edge
        // marker rather than a background repaint.
        let marker_x = find_symbol_x(buffer, first_row, 25, "▎");
        assert_eq!(
            buffer[(marker_x, first_row)].style().fg,
            Some(app.palette.accent)
        );
        assert!(
            !row_text(buffer, second_row, 25).contains('▎'),
            "the inactive row must not draw the active marker"
        );
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
            ratatui::style::Color::Reset,
            "active-but-not-selected workspace must not repaint its background (GC3)"
        );
        assert_eq!(
            buffer[(0, selected_row)].bg,
            app.palette.selection_bg,
            "navigate selection should use its dedicated cursor background"
        );
    }

    #[test]
    fn selected_active_workspace_resolves_expanded_background() {
        let mut app = crate::app::state::AppState::test_new();
        app.palette = crate::app::state::Palette::terminal();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.active = Some(0);
        app.selected = 0;
        app.mode = Mode::Navigate;
        let area = Rect::new(0, 0, 26, 20);
        app.view.workspace_card_areas = compute_workspace_card_areas(&app, area);
        let active_row = app.view.workspace_card_areas[0].rect.y;
        let inactive_row = app.view.workspace_card_areas[1].rect.y;
        let mut terminal = Terminal::new(TestBackend::new(26, 20)).unwrap();
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();

        assert_eq!(
            terminal.backend().buffer()[(0, active_row)].bg,
            app.palette.active_row_bg
        );

        app.selected = 1;
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(0, active_row)].bg,
            ratatui::style::Color::Reset,
            "active-but-not-selected workspace must not repaint its background (GC3)"
        );
        assert_eq!(
            terminal.backend().buffer()[(0, inactive_row)].bg,
            app.palette.selection_bg
        );

        app.palette = crate::app::state::Palette::catppuccin();
        app.selected = 0;
        terminal
            .draw(|frame| render_sidebar(&app, &TerminalRuntimeRegistry::new(), frame, area))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(0, active_row)].bg,
            app.palette.selection_bg
        );
    }

    #[test]
    fn selected_active_workspace_resolves_collapsed_background() {
        let mut app = crate::app::state::AppState::test_new();
        app.palette = crate::app::state::Palette::terminal();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.active = Some(0);
        app.selected = 0;
        app.mode = Mode::Navigate;
        let area = Rect::new(0, 0, 5, 8);
        let mut terminal = Terminal::new(TestBackend::new(5, 8)).unwrap();
        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .unwrap();

        let (workspace_area, _, _) = collapsed_sidebar_sections(area);
        assert_eq!(
            terminal.backend().buffer()[(workspace_area.x, workspace_area.y)].bg,
            app.palette.active_row_bg
        );

        app.selected = 1;
        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(workspace_area.x, workspace_area.y)].bg,
            app.palette.active_row_bg
        );
        assert_eq!(
            terminal.backend().buffer()[(workspace_area.x, workspace_area.y + 1)].bg,
            app.palette.selection_bg
        );

        app.palette = crate::app::state::Palette::catppuccin();
        app.selected = 0;
        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .unwrap();
        assert_eq!(
            terminal.backend().buffer()[(workspace_area.x, workspace_area.y)].bg,
            app.palette.selection_bg
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
            // GC3: active-but-not-selected no longer repaints the background.
            assert_eq!(style.bg, Some(ratatui::style::Color::Reset));
        }
        assert_eq!(separator.fg, Some(app.palette.overlay0));
        assert!(separator.add_modifier.contains(Modifier::DIM));
        assert!(!separator.add_modifier.contains(Modifier::BOLD));
        assert_eq!(separator.bg, Some(ratatui::style::Color::Reset));
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

        // Panel retired from the live layout (bora-49p.6); its rendering is
        // retained, so paint it directly into an area of our own.
        let agent_area = Rect::new(0, 0, 17, 10);
        let mut terminal = Terminal::new(TestBackend::new(17, 10)).unwrap();
        terminal
            .draw(|frame| {
                render_agent_detail(&app, &TerminalRuntimeRegistry::new(), frame, agent_area)
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
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

    /// The agent panel is retired (bora-49p.6): the workspace list takes the
    /// whole content column at every height, and the split ratio is ignored.
    #[test]
    fn expanded_sidebar_sections_give_the_whole_column_to_the_workspace_list() {
        for (rect, ratio) in [
            (Rect::new(0, 0, 20, 5), 0.9),
            (Rect::new(0, 0, 20, 5), 0.1),
            (Rect::new(0, 0, 26, 40), 0.5),
        ] {
            let (ws_area, detail_area) = expanded_sidebar_sections(rect, ratio);

            assert_eq!(
                ws_area,
                Rect::new(rect.x, rect.y, rect.width - 1, rect.height)
            );
            assert_eq!(detail_area.height, 0);
        }

        // Degenerate rects still yield nothing rather than a panic.
        assert_eq!(
            expanded_sidebar_sections(Rect::new(0, 0, 1, 5), 0.5),
            (Rect::default(), Rect::default())
        );
        assert_eq!(
            expanded_sidebar_sections(Rect::new(0, 0, 20, 0), 0.5),
            (Rect::default(), Rect::default())
        );
    }

    /// No second section means no drag handle, at any height.
    #[test]
    fn sidebar_section_divider_is_always_empty_while_the_panel_is_retired() {
        for height in [0u16, 5, 20, 40] {
            let divider = sidebar_section_divider_rect(Rect::new(0, 0, 20, height), 0.5);

            assert_eq!(divider, Rect::default(), "height {height}");
        }
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

    #[test]
    fn project_header_branch_label_truncates_with_explicit_ellipsis() {
        // The branch folded into a project header must truncate to an
        // explicit `…` sized to the row's chrome, never be hard-clipped.
        // Budget: width - ("╭─" + "herdr" + gap + brackets) = width - 10.
        let mut app = AppState::test_new();
        app.mouse_capture = false;
        app.active = None;
        let mut ws = git_space_member("main", "key-main", false);
        ws.cached_git_branch = Some("feature/branch-name-longer-than-sidebar".into());
        app.workspaces = vec![ws];
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        for (width, expected) in [
            (20u16, "╭─herdr [feature/b…]"),
            (30u16, "╭─herdr [feature/branch-name…]"),
            (40u16, "╭─herdr [feature/branch-name-longer-th…]"),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(width, 20)).expect("test terminal");
            terminal
                .draw(|frame| {
                    render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, width, 20), false)
                })
                .expect("workspace list should render");
            assert_eq!(
                row_text(terminal.backend().buffer(), 1, width),
                expected,
                "project header row at {width} cols"
            );
        }
    }

    #[test]
    fn branch_header_label_truncates_with_explicit_ellipsis() {
        // A branch sub-header must truncate its label to an explicit `…`
        // sized to the row's chrome, never be hard-clipped.
        // Budget: width - ("├── " connector) = width - 4.
        let mut app = AppState::test_new();
        app.mouse_capture = false;
        app.active = None;
        let identity = "github.com/owner/resume-builder";
        let mut first = git_space_member("main", "key-main", false);
        first.cached_git_branch = Some("first".into());
        let long = "feature/branch-name-longer-than-sidebar";
        let mut zebra = git_space_member("zebra", "key-zebra", false);
        zebra.cached_git_branch = Some(long.into());
        let mut yak = git_space_member("yak", "key-yak", false);
        yak.cached_git_branch = Some(long.into());
        for ws in [&mut first, &mut zebra, &mut yak] {
            ws.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        }
        app.workspaces = vec![first, zebra, yak];
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        for (width, expected) in [
            (20u16, "├── feature/branch-…"),
            (30u16, "├── feature/branch-name-longe…"),
            (40u16, "├── feature/branch-name-longer-than-sid…"),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(width, 20)).expect("test terminal");
            terminal
                .draw(|frame| {
                    render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, width, 20), false)
                })
                .expect("workspace list should render");
            assert_eq!(
                row_text(terminal.backend().buffer(), 3, width),
                expected,
                "branch header row at {width} cols"
            );
        }
    }

    /// Give a workspace's root pane a detected agent so it reports a pane
    /// detail (and thus an `@wNpN` badge) the way a live agent pane does.
    fn detect_agent_on_root_pane(app: &mut AppState, ws_idx: usize) {
        let pane = app.workspaces[ws_idx].tabs[0].root_pane;
        let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&terminal_id)
            .expect("ensure_test_terminals created the terminal")
            .detected_agent = Some(Agent::Pi);
    }

    #[test]
    fn indented_same_branch_children_render_distinct_badge_rows() {
        // Two workspaces on one checkout+branch used to render the same
        // repo-derived name under their header. An indented child row now
        // carries only its `@wNpN` pane badge (plus a branch only when the
        // parent header did not print it), so same-branch siblings stay
        // distinct. Exercises both parent shapes: the branch folded into
        // the project header and a `├──` branch sub-header.
        let mut app = AppState::test_new();
        app.mouse_capture = false;
        app.active = None;
        let identity = "github.com/owner/resume-builder";
        let mut first = git_space_member("first", identity, false);
        first.custom_name = None;
        first.cached_git_branch = Some("first".into());
        let mut second = git_space_member("second", identity, false);
        second.custom_name = None;
        second.cached_git_branch = Some("main".into());
        let mut third = git_space_member("third", identity, false);
        third.custom_name = None;
        third.cached_git_branch = Some("main".into());
        let ids = (first.id.clone(), second.id.clone(), third.id.clone());
        app.workspaces = vec![first, second, third];
        app.ensure_test_terminals();
        for ws_idx in 0..3 {
            detect_agent_on_root_pane(&mut app, ws_idx);
        }
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(30, 20)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 30, 20), false)
            })
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        // Project header with the first branch folded in, then its child:
        // the branch is already printed above, so the row is badge-only.
        assert_eq!(row_text(buffer, 1, 30).trim_end(), "╭─herdr [first]");
        assert_eq!(
            row_text(buffer, 2, 30).trim_end(),
            format!(" │   ◰  @{}p1", ids.0)
        );
        // Branch sub-header for the remaining branch, then its two children.
        assert_eq!(row_text(buffer, 3, 30).trim_end(), "├── main");
        let row5 = row_text(buffer, 4, 30);
        let row6 = row_text(buffer, 5, 30);
        assert_eq!(row5.trim_end(), format!(" │   ◰  @{}p1", ids.1));
        assert_eq!(row6.trim_end(), format!(" ╰── ◰  @{}p1", ids.2));
        assert_ne!(row5, row6, "same-branch siblings must render distinct rows");
    }

    #[test]
    fn indented_child_custom_name_renders_verbatim() {
        // A custom name is the user's own label: it passes through verbatim
        // and is never replaced by the badge-only child treatment.
        let mut app = AppState::test_new();
        app.mouse_capture = false;
        app.active = None;
        let identity = "github.com/owner/resume-builder";
        let mut auto = git_space_member("auto", identity, false);
        auto.custom_name = None;
        auto.cached_git_branch = Some("main".into());
        let mut named = git_space_member("named", identity, false);
        named.custom_name = Some("release-hotfix".into());
        named.cached_git_branch = Some("main".into());
        let auto_id = auto.id.clone();
        let named_id = named.id.clone();
        app.workspaces = vec![auto, named];
        app.ensure_test_terminals();
        for ws_idx in 0..2 {
            detect_agent_on_root_pane(&mut app, ws_idx);
        }
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(30, 20)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 30, 20), false)
            })
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        assert_eq!(row_text(buffer, 1, 30).trim_end(), "╭─herdr [main]");
        assert_eq!(
            row_text(buffer, 2, 30).trim_end(),
            format!(" │   ◰  @{}p1", auto_id)
        );
        assert_eq!(
            row_text(buffer, 3, 30).trim_end(),
            format!(" ╰── ◰ release-hotfix @{named_id}p1")
        );
    }

    #[test]
    fn indented_child_under_collapsed_header_shows_branch_header_omitted() {
        // A collapsed project header prints no branch, so its active child
        // row carries the branch itself — the one case where an indented
        // child's branch differs from what the header above printed.
        let mut app = AppState::test_new();
        app.mouse_capture = false;
        let identity = "github.com/owner/resume-builder";
        let mut active = git_space_member("active", identity, false);
        active.custom_name = None;
        active.cached_git_branch = Some("main".into());
        let mut other = git_space_member("other", identity, false);
        other.custom_name = None;
        other.cached_git_branch = Some("main".into());
        let active_id = active.id.clone();
        app.workspaces = vec![active, other];
        app.ensure_test_terminals();
        detect_agent_on_root_pane(&mut app, 0);
        app.active = Some(0);
        app.mode = Mode::Terminal;
        app.collapsed_space_keys.insert(identity.into());

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(30, 20)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 30, 20), false)
            })
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        // Collapsed header: no `[main]`, just the aggregate dot.
        assert_eq!(row_text(buffer, 1, 30).trim_end(), "╭─herdr ◰");
        // Active child: branch shown because the header above omitted it.
        assert_eq!(
            row_text(buffer, 2, 30).trim_end(),
            format!("▎ ◰ main @{active_id}p1")
        );
    }

    #[test]
    fn indented_child_without_agent_badge_keeps_display_name() {
        // A plain-shell pane reports no agent identity, so the row has no
        // `@wNpN` badge to carry it: such a child keeps its display name
        // instead of rendering an anonymous empty label.
        let mut app = AppState::test_new();
        app.mouse_capture = false;
        app.active = None;
        let identity = "github.com/owner/resume-builder";
        let mut first = git_space_member("first", identity, false);
        first.custom_name = None;
        first.cached_git_branch = Some("first".into());
        first.identity_cwd = std::path::PathBuf::from("/repo/first");
        let mut second = git_space_member("second", identity, false);
        second.custom_name = None;
        second.cached_git_branch = Some("main".into());
        second.identity_cwd = std::path::PathBuf::from("/repo/second");
        let mut third = git_space_member("third", identity, false);
        third.custom_name = None;
        third.cached_git_branch = Some("main".into());
        third.identity_cwd = std::path::PathBuf::from("/repo/third");
        // No detected agents: no workspace has a pane badge.
        app.workspaces = vec![first, second, third];
        app.ensure_test_terminals();
        app.mode = Mode::Terminal;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal = Terminal::new(TestBackend::new(30, 20)).expect("test terminal");
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 30, 20), false)
            })
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        // Badge-less children fall back to the cwd-derived display name.
        assert_eq!(row_text(buffer, 2, 30).trim_end(), " │   ◰ first");
        assert_eq!(row_text(buffer, 4, 30).trim_end(), " │   ◰ second");
        assert_eq!(row_text(buffer, 5, 30).trim_end(), " ╰── ◰ third");
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
    fn view_mode_toggle_rect_is_right_aligned_and_sized_to_label() {
        let area = Rect::new(0, 0, 40, 5);
        let rect = view_mode_toggle_rect(area, crate::config::ViewMode::Project);
        let label = crate::config::ViewMode::Project.as_str();
        assert_eq!(rect.height, 1, "one row tall");
        assert_eq!(rect.y, area.y, "sits on the area's top row");
        assert_eq!(
            rect.width,
            display_width_u16(label),
            "sized exactly to the label"
        );
        assert_eq!(
            rect.x + rect.width,
            area.x + area.width,
            "right-aligned: flush with the area's trailing edge"
        );

        assert_eq!(
            view_mode_toggle_rect(Rect::new(0, 0, 0, 5), crate::config::ViewMode::Flat),
            Rect::default(),
            "zero width -> no rect"
        );
        assert_eq!(
            view_mode_toggle_rect(Rect::new(0, 0, 40, 0), crate::config::ViewMode::Flat),
            Rect::default(),
            "zero height -> no rect"
        );
    }

    #[test]
    fn workspace_list_shows_view_mode_toggle_not_spaces_title() {
        // bora regression fix: commit 7bb8133b dropped both the ` spaces`
        // title and the view-mode toggle when it only meant to drop the
        // title. The toggle is restored; the title stays gone.
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        app.workspaces = vec![Workspace::test_new("alpha")];

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 30, 10);
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        let full: String = (0..area.height)
            .map(|y| row_text(buffer, y, area.width))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !full.contains("spaces"),
            "the ` spaces` title must stay gone: {full:?}"
        );
        let top_row = row_text(buffer, 0, area.width);
        assert!(
            top_row.trim_end().ends_with("project"),
            "current view-mode name renders right-aligned on the list's top margin row: {top_row:?}"
        );
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

        // The channel group holds both channels, and its rail closes on the
        // last. Column 0 is the active-row marker lane (blank here, since
        // nothing is active) — the rail itself now starts at column 1.
        let channel_rows = &rows[group_at + 1..group_at + 3];
        assert!(
            channel_rows[0].starts_with(" │") && channel_rows[0].contains("#canal-ary"),
            "first channel rides the spine: {channel_rows:?}"
        );
        assert!(
            channel_rows[1].starts_with(" ╰── ") && channel_rows[1].contains("#part3-model-status"),
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
        app.view_mode = crate::config::ViewMode::Flat;
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
        assert_eq!(app.view_mode, crate::config::ViewMode::Repo);
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

        app.view_mode = crate::config::ViewMode::Flat;
        let flat = workspace_list_entries(&app);
        assert!(flat
            .iter()
            .all(|e| matches!(e, WorkspaceListEntry::Workspace { .. })));

        // Simulate the drag reorder a user performs while flat.
        app.workspaces.swap(0, 1);

        app.view_mode = crate::config::ViewMode::Repo;
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

    /// Adversarial counterpart of `git_space_member`: base state comes from
    /// `Workspace::test_adversarial_identity_state()` (raw pane id != public
    /// pane number, a closed pane still holding stale bookkeeping, active
    /// tab position != public tab number) instead of a pristine
    /// `Workspace::test_new`, so grouping/lockstep code below is exercised
    /// against pathological internal workspace state, not just tidy ones.
    fn adversarial_git_space_member(
        key: &str,
        is_linked_worktree: bool,
    ) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::test_adversarial_identity_state();
        ws.cached_git_branch = None;
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: key.into(),
            repo_identity: key.into(),
            checkout_key: format!("/repo/{key}"),
            repo_name: "herdr".into(),
            repo_root: std::path::PathBuf::from("/repo/herdr"),
            is_linked_worktree,
        });
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
        let body_y = WORKSPACE_LIST_TOP_MARGIN_ROWS;
        assert!(
            row_text(body_y).starts_with("╭─") && row_text(body_y).contains("feat/a"),
            "header opens bracket with folded branch: {:?}",
            row_text(body_y)
        );
        assert!(
            row_text(body_y + 1).starts_with(" │"),
            "folded member on spine: {:?}",
            row_text(body_y + 1)
        );
        assert!(
            row_text(body_y + 2).starts_with(" │"),
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
        assert_eq!(entry_row_height(&entries[0], &entries, 0, 0), 1);
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
        assert_eq!(entry_row_height(&entries[0], &entries, 0, 0), 1);
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
        assert_eq!(entry_row_height(&entries[0], &entries, 0, 0), 1);
        assert_eq!(entry_row_height(&entries[1], &entries, 1, 0), 1);
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
                WorkspaceListEntry::ProjectRow { .. }
                | WorkspaceListEntry::WorktreeRow { .. }
                | WorkspaceListEntry::SectionRow { .. }
                | WorkspaceListEntry::SectionHeader { .. }
                | WorkspaceListEntry::SectionItem { .. }
                | WorkspaceListEntry::PrRow { .. }
                | WorkspaceListEntry::PaneDotsRow { .. } => {
                    panic!("repo-view fixture must never emit a project-view entry")
                }
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
            .map(|(idx, entry)| entry_row_height(entry, &entries, idx, 0))
            .sum();
        assert_eq!(total_height, 4, "1+1+1+1 rows for the pinned sequence");

        // Visible-count pass agrees: a body exactly `total_height` rows tall
        // shows every entry; one row less drops exactly the last (1-row)
        // entry. Section area height = body + header rows + footer row
        // (the Programs band reservation is gone since bora-55c.3).
        let exact = Rect::new(0, 0, 30, total_height + WORKSPACE_LIST_TOP_MARGIN_ROWS + 1);
        assert_eq!(workspace_list_visible_count(&app, exact, 0), entries.len());
        let short = Rect::new(0, 0, 30, total_height + WORKSPACE_LIST_TOP_MARGIN_ROWS);
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
                WorkspaceListEntry::ProjectRow { .. }
                | WorkspaceListEntry::WorktreeRow { .. }
                | WorkspaceListEntry::SectionRow { .. }
                | WorkspaceListEntry::SectionHeader { .. }
                | WorkspaceListEntry::SectionItem { .. }
                | WorkspaceListEntry::PrRow { .. }
                | WorkspaceListEntry::PaneDotsRow { .. } => {
                    panic!("repo-view fixture must never emit a project-view entry")
                }
            }
            y += entry_row_height(entry, &entries, idx, 0);
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
        let body_y = WORKSPACE_LIST_TOP_MARGIN_ROWS; // exact rect starts at y = 0
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

    // Exhaustive counterpart to `workspace_list_lockstep_passes_agree_for_git_repo_group`:
    // that test pins ONE shape (a git repo group producing ProjectHeader +
    // Workspace only). This test instead builds a fixture that forces every
    // `WorkspaceListEntry` variant to appear at least once -- GroupHeader (a
    // user visual group), ProjectHeader + Workspace (a git repo group),
    // BranchHeader both folded (`ws_idx: Some`, a single auto-named branch
    // member) and plain (`ws_idx: None`, two members on one branch), and
    // HiddenHeader (a hidden workspace) -- and every workspace involved comes
    // from `Workspace::test_adversarial_identity_state()` rather than a
    // pristine fixture, so the lockstep contract is checked against
    // pathological internal state, not just tidy ones.
    //
    // The THREE lockstep passes named at sidebar.rs:703-705 are
    // `workspace_list_visible_count` (visible-count pass),
    // `compute_workspace_list_areas` (geometry pass), and
    // `render_workspace_list` (render pass); all three MUST consume exactly
    // `entry_row_height` rows per entry, for every entry, regardless of
    // variant. This test checks all three against the same
    // `workspace_list_entries(&app)` sequence.
    //
    // Exhaustiveness is enforced BY THE COMPILER, not a hand-maintained list:
    // every `match entry { .. }` below is a non-wildcard match over
    // `WorkspaceListEntry`. Adding a variant to the enum without adding an
    // arm to every match here fails to compile.
    #[test]
    fn workspace_list_lockstep_passes_agree_for_every_entry_variant() {
        let mut app = AppState::test_with_adversarial_identity_state();
        // app.workspaces[0] is the adversarial base workspace: no git space,
        // no visual group, not hidden -> renders as a flat `Workspace` entry.
        // Every workspace that reaches a `Workspace` row gets a custom_name,
        // because the render pass below asserts that name verbatim: a row
        // without one renders a *derived* label (the repo-derived display name,
        // or — for an indented child, since bora-rlu.2 — its `@wNpN` badge),
        // and asserting that would mean re-implementing the renderer inside its
        // own test. Naming them costs no coverage: branch folding turns on
        // auto-naming only for a LONE member, so the two-member branch below
        // stays uncollapsed either way.
        app.workspaces[0].custom_name = Some("adv-base".into());

        let identity = "github.com/owner/adversarial-proj";

        // Git repo group: parent (no branch, rides inside the bracket),
        // branch-a (folds into the ProjectHeader's own `branch` field),
        // branch-b (single auto-named member -> BranchHeader{ws_idx: Some}),
        // branch-c (two auto-named members -> BranchHeader{ws_idx: None} + 2
        // Workspace rows -- 2+ members must never collapse).
        let mut parent = adversarial_git_space_member("key-parent", false);
        parent.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        parent.custom_name = Some("adv-parent".into());

        let mut branch_a = adversarial_git_space_member("key-a", false);
        branch_a.cached_git_branch = Some("feat/a".into());
        branch_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        branch_a.custom_name = Some("adv-branch-a".into());

        let mut branch_b = adversarial_git_space_member("key-b", false);
        branch_b.cached_git_branch = Some("feat/b".into());
        branch_b.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        branch_b.custom_name = None; // lone auto-named member -> folds into its BranchHeader

        let mut branch_c1 = adversarial_git_space_member("key-c1", true);
        branch_c1.cached_git_branch = Some("feat/c".into());
        branch_c1.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        branch_c1.custom_name = Some("adv-branch-c1".into());
        let mut branch_c2 = adversarial_git_space_member("key-c2", true);
        branch_c2.cached_git_branch = Some("feat/c".into());
        branch_c2.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        branch_c2.custom_name = Some("adv-branch-c2".into()); // 2 members on one branch must never collapse

        // Visual group: a lone member renders GroupHeader + indented Workspace.
        let mut vg_member = Workspace::test_adversarial_identity_state();
        vg_member.cached_git_branch = None;
        vg_member.custom_name = Some("adv-vg-member".into());
        vg_member.visual_group = Some("adv-group".into());

        // Hidden workspace: its row disappears, folded into the trailing
        // HiddenHeader count instead.
        let mut hidden_ws = Workspace::test_adversarial_identity_state();
        hidden_ws.cached_git_branch = None;
        hidden_ws.custom_name = Some("adv-hidden".into());

        app.workspaces.extend([
            parent, branch_a, branch_b, branch_c1, branch_c2, vg_member, hidden_ws,
        ]);
        let hidden_idx = app.workspaces.len() - 1;
        let hide_key = AppState::workspace_hide_key(&app.workspaces[hidden_idx]);
        app.hidden_space_keys.insert(
            hide_key,
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        );
        app.ensure_test_terminals();

        let entries = workspace_list_entries(&app);

        // --- By-construction exhaustiveness: every variant must appear at
        // least once, or the fixture above needs extending. ---
        #[derive(Default, Debug)]
        struct VariantsSeen {
            workspace: bool,
            group_header: bool,
            project_header: bool,
            branch_header_folded: bool,
            branch_header_plain: bool,
            hidden_header: bool,
        }
        let mut seen = VariantsSeen::default();
        for entry in &entries {
            match entry {
                WorkspaceListEntry::Workspace { .. } => seen.workspace = true,
                WorkspaceListEntry::GroupHeader { .. } => seen.group_header = true,
                WorkspaceListEntry::ProjectHeader { .. } => seen.project_header = true,
                WorkspaceListEntry::BranchHeader { ws_idx, .. } => {
                    if ws_idx.is_some() {
                        seen.branch_header_folded = true;
                    } else {
                        seen.branch_header_plain = true;
                    }
                }
                WorkspaceListEntry::HiddenHeader { .. } => seen.hidden_header = true,
                WorkspaceListEntry::ProjectRow { .. }
                | WorkspaceListEntry::WorktreeRow { .. }
                | WorkspaceListEntry::SectionRow { .. }
                | WorkspaceListEntry::SectionHeader { .. }
                | WorkspaceListEntry::SectionItem { .. }
                | WorkspaceListEntry::PrRow { .. }
                | WorkspaceListEntry::PaneDotsRow { .. } => {
                    panic!("repo-view fixture must never emit a project-view entry")
                }
            }
        }
        assert!(
            seen.workspace,
            "fixture must produce a Workspace entry: {entries:#?}"
        );
        assert!(
            seen.group_header,
            "fixture must produce a GroupHeader entry: {entries:#?}"
        );
        assert!(
            seen.project_header,
            "fixture must produce a ProjectHeader entry: {entries:#?}"
        );
        assert!(
            seen.branch_header_folded,
            "fixture must produce a BranchHeader{{ws_idx: Some}} entry: {entries:#?}"
        );
        assert!(
            seen.branch_header_plain,
            "fixture must produce a BranchHeader{{ws_idx: None}} entry: {entries:#?}"
        );
        assert!(
            seen.hidden_header,
            "fixture must produce a HiddenHeader entry: {entries:#?}"
        );

        // --- Pass 1: height. Shared per-entry height helper. ---
        let total_height: u16 = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| entry_row_height(entry, &entries, idx, 0))
            .sum();

        // --- Pass 2: visible-count. An exact-fit body shows every entry; one
        // row less drops exactly the last entry (every current variant is 1
        // row tall, so "one row" is "one entry" -- if a future change makes
        // any variant taller without updating `entry_row_height`, either
        // this or pass 3/4 below catches the drift). The body reserves only
        // the one footer row below the list since bora-55c.3 removed the
        // Programs band reservation, so exact-fit slack is 1, not 2. ---
        let width = 60;
        let exact = Rect::new(
            0,
            0,
            width,
            total_height + WORKSPACE_LIST_TOP_MARGIN_ROWS + 1,
        );
        assert_eq!(workspace_list_visible_count(&app, exact, 0), entries.len());
        let short = Rect::new(0, 0, width, total_height + WORKSPACE_LIST_TOP_MARGIN_ROWS);
        assert_eq!(
            workspace_list_visible_count(&app, short, 0),
            entries.len() - 1
        );

        // --- Pass 3: geometry. Card/header rects sit at the prefix sums of
        // `entry_row_height`, generically for every entry regardless of
        // variant (a `BranchHeader{ws_idx: None}` contributes no area of its
        // own but must still advance `row_y` by its `entry_row_height`, or
        // every card/header after it would land on the wrong row -- which is
        // exactly what these two `assert_eq!` calls would catch). ---
        let sidebar = Rect::new(
            0,
            0,
            width,
            total_height + WORKSPACE_LIST_TOP_MARGIN_ROWS + 20,
        );
        let (cards, headers) = compute_workspace_list_areas(&app, sidebar);
        let ws_area = workspace_list_rect(sidebar, app.sidebar_section_split);
        let body = workspace_list_body_rect(&app, ws_area, false);
        let mut expected_cards: Vec<(usize, u16)> = Vec::new();
        let mut expected_headers: Vec<(String, u16)> = Vec::new();
        let mut y = body.y;
        for (idx, entry) in entries.iter().enumerate() {
            match entry {
                WorkspaceListEntry::Workspace { ws_idx, .. } => expected_cards.push((*ws_idx, y)),
                WorkspaceListEntry::BranchHeader {
                    ws_idx: Some(idx2), ..
                } => expected_cards.push((*idx2, y)),
                WorkspaceListEntry::BranchHeader { ws_idx: None, .. } => {}
                WorkspaceListEntry::GroupHeader { collapse_key, .. }
                | WorkspaceListEntry::ProjectHeader { collapse_key, .. } => {
                    expected_headers.push((collapse_key.clone(), y))
                }
                WorkspaceListEntry::HiddenHeader { .. } => {
                    expected_headers.push(("hidden:".to_string(), y))
                }
                WorkspaceListEntry::ProjectRow { .. }
                | WorkspaceListEntry::WorktreeRow { .. }
                | WorkspaceListEntry::SectionRow { .. }
                | WorkspaceListEntry::SectionHeader { .. }
                | WorkspaceListEntry::SectionItem { .. }
                | WorkspaceListEntry::PrRow { .. }
                | WorkspaceListEntry::PaneDotsRow { .. } => {
                    panic!("repo-view fixture must never emit a project-view entry")
                }
            }
            y += entry_row_height(entry, &entries, idx, 0);
        }
        assert_eq!(y - body.y, total_height);
        assert_eq!(
            cards
                .iter()
                .map(|c| (c.ws_idx, c.rect.y))
                .collect::<Vec<_>>(),
            expected_cards
        );
        assert_eq!(
            headers
                .iter()
                .map(|h| (h.collapse_key.clone(), h.rect.y))
                .collect::<Vec<_>>(),
            expected_headers
        );

        // --- Pass 4: render. Every entry's own distinguishing text lands on
        // the same prefix-sum row the height/geometry passes computed above --
        // generically, per entry, not just for one hand-picked row. ---
        let mut terminal =
            Terminal::new(TestBackend::new(exact.width, exact.height)).expect("test terminal");
        let runtimes = TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, exact, false))
            .expect("workspace list should render");
        let body_y = WORKSPACE_LIST_TOP_MARGIN_ROWS; // exact rect starts at y = 0
        let mut y = body_y;
        for (idx, entry) in entries.iter().enumerate() {
            let expected_substr: String = match entry {
                WorkspaceListEntry::Workspace { ws_idx, .. } => app.workspaces[*ws_idx]
                    .custom_name
                    .clone()
                    .expect("fixture sets custom_name on every workspace used here"),
                WorkspaceListEntry::GroupHeader { name, .. } => name.clone(),
                WorkspaceListEntry::ProjectHeader { name, .. } => name.clone(),
                WorkspaceListEntry::BranchHeader { label, .. } => label.clone(),
                WorkspaceListEntry::HiddenHeader { .. } => "Hidden".to_string(),
                WorkspaceListEntry::ProjectRow { .. }
                | WorkspaceListEntry::WorktreeRow { .. }
                | WorkspaceListEntry::SectionRow { .. }
                | WorkspaceListEntry::SectionHeader { .. }
                | WorkspaceListEntry::SectionItem { .. }
                | WorkspaceListEntry::PrRow { .. }
                | WorkspaceListEntry::PaneDotsRow { .. } => {
                    panic!("repo-view fixture must never emit a project-view entry")
                }
            };
            let actual = row_text(terminal.backend().buffer(), y, exact.width);
            assert!(
                actual.contains(&expected_substr),
                "entry {idx} ({entry:?}) expected {expected_substr:?} at row {y}, got {actual:?}"
            );
            y += entry_row_height(entry, &entries, idx, 0);
        }

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

    // ── Project-view row painters + geometry (bora-49p.3) ────────────────

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn project_row_line_right_aligns_counter_and_fits_width() {
        let p = Palette::catppuccin();
        let width = 42;
        // Before this fix `project_row_line` took no `collapsed` param and
        // never drew a chevron; the width invariant only had one shape to
        // hold. A collapsed group now gets a chevron back (owner's ask,
        // item 3b) that eats into the name's own budget rather than
        // padding out the fixed width, so the invariant is asserted in
        // BOTH states here. T7 (bora-79l, divergence F): the title starts
        // with the 1-column gutter (` CNB`, ALVO_CAPTURE row 01) — fica
        // vermelho se o nome voltar à coluna 0.
        let expanded = line_text(&project_row_line("CNB", 3, 4, false, &p, width));
        assert_eq!(display_width(&expanded), width as usize);
        // bora-c1h G1: the hexagon is gone — the gutter, then the group
        // name, underlined.
        assert!(expanded.starts_with(" CNB"));
        assert!(
            !expanded.contains('⬢'),
            "no hexagon on the group header: {expanded:?}"
        );
        assert!(expanded.ends_with("3/4"));

        let collapsed = line_text(&project_row_line("CNB", 3, 4, true, &p, width));
        assert_eq!(
            display_width(&collapsed),
            width as usize,
            "the width invariant holds with the caret too: {collapsed:?}"
        );
        assert!(
            collapsed.starts_with(" ▸ CNB"),
            "a collapsed group gets its caret back, after the gutter: {collapsed:?}"
        );
        assert!(collapsed.ends_with("3/4"));
    }

    #[test]
    fn project_row_line_has_no_separator_rule_before_the_counter() {
        // Ground-truth re-approval: the approved mock's `.g` rule draws no
        // ruler at all — Solo #11's dash-fill was a deviation from the
        // approved design, not the source of truth. The gap between the
        // name and the counter is now plain space, still padded to width.
        let p = Palette::catppuccin();
        let width = 30;
        let text = line_text(&project_row_line("CNB", 1, 4, false, &p, width));

        assert_eq!(display_width(&text), width as usize, "row: {text:?}");
        assert_eq!(
            text.chars().filter(|&c| c == '─').count(),
            0,
            "no ruler on the group header: {text:?}"
        );
        assert!(text.trim_end().ends_with("1/4"));
    }

    #[test]
    fn section_header_ruler_fills_exact_width_to_the_counter_column() {
        let p = Palette::catppuccin();
        let width = 40;
        let text = line_text(&section_header_line(&COMMANDS, 1, 3, None, &p, width));

        // Row is loaded exactly to `width`, not merely "wide enough" — a
        // leader/counter budget mismatch shows up as drift here. T7
        // (bora-79l): the leader is the same `·` the branch headers use
        // and the counter sits FLUSH against its last dot (ALVO_CAPTURE
        // row 31: ` ≡ COMANDO ·····0/1`) — fica vermelho se voltar o
        // `─` ruler, o indent de 4, ou o espaço antes do contador.
        assert_eq!(display_width(&text), width as usize, "row: {text:?}");
        assert!(text.trim_end().ends_with("1/3"));
        let dot_run = text.chars().filter(|&c| c == '·').count();
        assert!(dot_run > 0, "dotted leader must exist: {text:?}");
        let prefix = " ≡ COMANDO ";
        let counter = "1/3";
        let expected_dots = width as usize - display_width(prefix) - display_width(counter);
        assert_eq!(dot_run, expected_dots, "row: {text:?}");
    }

    #[test]
    fn section_header_checks_glyph_differs_from_commands() {
        let p = Palette::catppuccin();
        let commands = line_text(&section_header_line(&COMMANDS, 0, 2, None, &p, 30));
        let checks = line_text(&section_header_line(&CHECKS, 2, 2, None, &p, 30));

        // T7 (bora-79l): this pinned that the two bands' glyphs differed
        // (≡ vs ✓). ALVO_CAPTURE rows 31/33 pin `≡` for BOTH — the ✓ was
        // an old rollup echo — so the pin flips: fica vermelho se CHECKS
        // voltar a um glifo próprio (ou COMANDO perder o label novo);
        // as linhas continuam distintas pelos labels.
        assert!(commands.starts_with(" ≡ COMANDO"));
        assert!(checks.starts_with(" ≡ CHECKS"));
        assert_ne!(commands, checks);
    }
    #[test]
    // T7 (bora-79l): counter flush against the `·` leader now — fica
    // vermelho se o contador voltar com espaço à frente (` 2`).
    fn section_header_notes_shows_plain_count_not_a_progress_ratio() {
        let p = Palette::catppuccin();
        let notes = line_text(&section_header_line(&NOTES, 0, 2, None, &p, 30));
        let todos = line_text(&section_header_line(&TODOS, 1, 3, None, &p, 30));

        assert!(notes.contains("NOTES"));
        assert!(
            notes.trim_end().ends_with("·2"),
            "doc count flush against the leader, no slash: {notes:?}"
        );
        assert!(
            !notes.contains("0/2"),
            "NOTES is not a progress bar: {notes:?}"
        );
        assert!(todos.contains("TODOS"));
        assert!(todos.trim_end().ends_with("1/3"));
    }

    #[test]
    fn worktree_row_omits_repo_name_when_single_repo_project() {
        let p = Palette::catppuccin();
        let text = line_text(&worktree_row_line(
            None, "main", 0, 0, None, false, false, &p, 40,
        ));

        assert_eq!(text, "  ▾ main");
    }

    #[test]
    fn worktree_row_shows_repo_name_when_project_spans_repos() {
        let p = Palette::catppuccin();
        let text = line_text(&worktree_row_line(
            Some("cnb_landing_page"),
            "main",
            0,
            0,
            None,
            false,
            false,
            &p,
            40,
        ));

        assert_eq!(text, "  ▾ cnb_landing_page  main");
    }

    #[test]
    fn worktree_row_truncates_branch_without_overlapping_the_pr_badge() {
        let p = Palette::catppuccin();
        let width = 40;
        let long_branch = "feature/very-long-branch-name-that-does-not-fit";
        let text = line_text(&worktree_row_line(
            Some("cnb_landing_page"),
            long_branch,
            0,
            0,
            Some(128),
            false,
            false,
            &p,
            width,
        ));

        assert!(
            display_width(&text) <= width as usize,
            "row must respect its width budget, got {text:?}"
        );
        assert!(
            text.contains('…'),
            "long branch must be truncated: {text:?}"
        );
        assert!(
            text.trim_end().ends_with("#128"),
            "PR badge must survive truncation intact: {text:?}"
        );
        assert!(
            !text.contains(long_branch),
            "full untruncated branch must not appear: {text:?}"
        );
    }

    #[test]
    fn worktree_row_unopened_renders_dimmed_branch() {
        let p = Palette::catppuccin();
        let normal = worktree_row_line(None, "main", 0, 0, None, false, false, &p, 40);
        let unopened = worktree_row_line(None, "main", 0, 0, None, false, true, &p, 40);

        let normal_style = normal
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "main")
            .expect("normal row has a branch span")
            .style;
        let unopened_style = unopened
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "main")
            .expect("unopened row has a branch span")
            .style;

        assert!(!normal_style.add_modifier.contains(Modifier::DIM));
        assert!(unopened_style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn section_item_line_shows_bullet_label_and_right_aligned_detail() {
        let p = Palette::catppuccin();
        let running = line_text(&section_item_line(
            &COMMANDS,
            "dev",
            Some(":5173"),
            true,
            &p,
            40,
        ));
        let idle = line_text(&section_item_line(&COMMANDS, "test", None, false, &p, 40));

        assert!(running.trim_start().starts_with('●'));
        assert!(running.trim_end().ends_with(":5173"));
        assert!(idle.trim_start().starts_with('·'));
    }

    #[test]
    fn checks_row_line_marks_failures_with_a_red_cross() {
        let p = Palette::catppuccin();
        let failing = section_item_line(&CHECKS, "clippy", None, false, &p, 40);
        let text = line_text(&failing);

        assert!(
            text.trim_start().starts_with('✗'),
            "a failing CHECKS row must read as a failure, got {text:?}"
        );
        assert!(text.contains("clippy"));
        let bullet = failing
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "✗")
            .expect("checks row has a ✗ bullet");
        assert_eq!(bullet.style.fg, Some(p.red));
    }

    // ── bora-c1h: v3 section row (G1-G5) ────────────────────────────────

    fn unicode_glyphs() -> crate::config::ProjectGlyphs {
        crate::config::project_glyphs(crate::config::sidebar::SidebarGlyphStyle::Unicode)
    }

    #[test]
    fn project_row_line_has_no_hexagon_and_is_underlined() {
        let p = Palette::catppuccin();
        let line = project_row_line("Bora", 1, 2, false, &p, 40);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!text.contains('⬢'), "G1: no hexagon glyph: {text:?}");
        let name_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Bora")
            .expect("name span must render verbatim");
        assert!(
            name_span.style.add_modifier.contains(Modifier::UNDERLINED),
            "G1: the group name must be underlined: {:?}",
            name_span.style
        );
        // Item 6: before this fix the header was BOLD | UNDERLINED. It now
        // claims ITALIC | UNDERLINED — ITALIC is the header's own
        // face-selection channel, uncontested by `section_row_line`'s
        // branch label (which owns BOLD|ITALIC for its own distinct face).
        // BOLD is deliberately dropped: the row's own slightly-lighter
        // background (item 3c) carries the emphasis instead, per the
        // owner's own call ("I don't think we even need the Bold if we had
        // the background").
        assert!(
            name_span.style.add_modifier.contains(Modifier::ITALIC),
            "the header claims the plain-italic face channel: {:?}",
            name_span.style
        );
        assert!(
            !name_span.style.add_modifier.contains(Modifier::BOLD),
            "BOLD must stay off — the row's own background supplies the emphasis now: {:?}",
            name_span.style
        );
        assert_eq!(
            name_span.style.fg,
            Some(p.mauve),
            "ground-truth re-approval: the group header accent is mauve: {:?}",
            name_span.style
        );
        assert!(text.contains("1/2"), "count stays right-aligned: {text:?}");
    }

    #[test]
    fn section_row_line_declares_the_branch_without_name_or_chevron() {
        // Attribution (T3, bora-79l): this was
        // `section_row_line_shows_bright_uppercase_name_and_dim_branch` —
        // it pinned the UPPERCASE repo-name slot at `p.text` BOLD and a
        // BOLD|ITALIC|DIM branch riding the Ghostty font-selection
        // channel. The declared-branch header removed the name slot by
        // assignment (the workspace's name lives on its `PaneDotsRow`;
        // the P1 double-print dies here), so the row's only text is the
        // branch label: overlay1 + BOLD, no DIM, no ITALIC — the
        // font-selection channel is retired with the slot that used it.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let line = section_row_line(
            false,
            Some("feature/x"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            60,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            !text.contains("FEATURE-X") && !text.chars().any(char::is_uppercase),
            "no name slot, nothing uppercase — the header declares a branch: {text:?}"
        );
        assert!(
            text.contains("feature/x"),
            "the branch label is the row's whole text: {text:?}"
        );
        assert!(
            !text.contains('▾') && !text.contains('▸'),
            "no chevron — collapse belongs to the folder (ProjectRow): {text:?}"
        );
        let branch_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref().contains("feature/x"))
            .expect("branch span");
        assert_eq!(
            branch_span.style.fg,
            Some(p.overlay1),
            "the label is overlay1 — recessive without DIM tricks: {branch_span:?}"
        );
        assert!(
            branch_span.style.add_modifier.contains(Modifier::BOLD),
            "the label is BOLD: {branch_span:?}"
        );
        assert!(
            !branch_span.style.add_modifier.contains(Modifier::DIM | Modifier::ITALIC),
            "no DIM, no ITALIC — the font-selection channel died with the name slot: {branch_span:?}"
        );
    }

    #[test]
    fn section_row_line_marks_worktree_checkouts_with_hilbert_glyph_main_gets_none() {
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let worktree = line_text(&section_row_line(
            true,
            Some("fix/x"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            60,
        ));
        let main = line_text(&section_row_line(
            false,
            Some("main"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            60,
        ));
        assert!(
            worktree.contains('⌗'),
            "G4: worktree sections get ⌗: {worktree:?}"
        );
        assert!(!main.contains('⌗'), "G4: main checkouts get no ⌗: {main:?}");
        assert!(
            !worktree.contains("##"),
            "G4: no condensed ## prefix: {worktree:?}"
        );
    }

    #[test]
    fn section_row_line_cluster_never_zero_widths_the_branch() {
        // Attribution (T3): was `..._never_zero_widths_the_name` — the
        // budget priority survives (cluster reserved in full first, the
        // label ellipsizes into whatever is left), only the thing that
        // truncates changed: the branch label, since the name slot is
        // gone. The `✱`/`±` dirty/staged glyph assertions retired with
        // the glyphs themselves — the numeric `+N −M` diff subsumes them.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let line = section_row_line(
            false,
            Some("feature/very-long-branch-name-too"),
            Some((916, 2)),
            3,
            5,
            Some((74, PrChipTone::Open)),
            Some(crate::workspace::ChecksRollup::Failing),
            &glyphs,
            &p,
            40,
        );
        let text = line_text(&line);
        assert!(
            text.contains("+916 −2"),
            "diff numbers survive truncation: {text:?}"
        );
        assert!(
            text.contains("↑3"),
            "ahead glyph survives truncation: {text:?}"
        );
        assert!(
            text.contains("↓5"),
            "behind glyph survives truncation: {text:?}"
        );
        assert!(
            text.contains("PR74"),
            "PR chip survives truncation: {text:?}"
        );
        assert!(
            text.contains('✗'),
            "checks glyph survives truncation: {text:?}"
        );
        assert!(
            display_width(&text) <= 40,
            "row must not exceed the given width: {text:?}"
        );
    }

    #[test]
    fn section_row_line_pins_the_state_cluster_to_the_right_edge() {
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        // A short label leaves slack: the declared header floats the
        // cluster right on a dotted leader, so the row must fill to
        // `width` and end on the cluster.
        let line = section_row_line(false, Some("ws"), None, 3, 0, None, None, &glyphs, &p, 40);
        let text = line_text(&line);
        assert_eq!(
            display_width(&text),
            40,
            "a row with a cluster fills to the right edge: {text:?}"
        );
        assert!(
            text.ends_with("↑3"),
            "the cluster is the last thing on the row: {text:?}"
        );
        // No cluster: no leader, no padding, the row stays short.
        let plain = line_text(&section_row_line(
            false,
            Some("ws"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            40,
        ));
        assert!(
            display_width(&plain) < 40 && !plain.contains('·'),
            "a clusterless row is not padded and draws no leader: {plain:?}"
        );
    }

    #[test]
    fn section_row_line_cluster_is_all_gray_per_r1() {
        // Attribution (T3): was `..._ahead_green_behind_and_dirty_and_
        // staged_yellow`. The owner's R1 color budget (2026-08-27)
        // reassigns every git-plumbing hue to gray — green means
        // "answered/ready" (a pane state), and the old yellow
        // behind/dirty/staged markers competed with the one red that
        // matters (a failing check). The dirty/staged `✱`/`±` glyphs are
        // gone entirely, subsumed by the numeric diff.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let line = section_row_line(
            false,
            Some("main"),
            Some((4, 2)),
            1,
            1,
            None,
            None,
            &glyphs,
            &p,
            60,
        );
        let find = |glyph: &str| {
            line.spans
                .iter()
                .find(|s| s.content.as_ref().contains(glyph))
                .unwrap_or_else(|| panic!("expected a span containing {glyph:?}: {line:?}"))
        };
        assert_eq!(find("+4").style.fg, Some(p.overlay1), "diff is gray");
        assert_eq!(find("↑1").style.fg, Some(p.overlay1), "ahead is gray");
        assert_eq!(find("↓1").style.fg, Some(p.overlay1), "behind is gray");
        assert!(
            !line_text(&line).contains('✱') && !line_text(&line).contains('±'),
            "the dirty/staged glyphs are subsumed by the numeric diff"
        );
    }

    #[test]
    // T7 (bora-79l): the checks-glyph span is trimmed to bare `✓/✗/●`
    // (push_cluster owns all cluster spacing now) — fica vermelho se o
    // glifo voltar com espaço colado ao span.
    fn pr_chip_prints_gray_checks_glyph_keeps_rollup() {
        // Attribution (T3): was `pr_chip_follows_github_state_colors` —
        // merged purple / closed red / draft dim / open rollup-colored.
        // R1 kills every one of those: the chip is a counter, and
        // counters never shout; the ONLY colored thing in the cluster is
        // the checks glyph, still owned by `checks_rollup_glyph` (the
        // red-on-failing rule itself is pinned by
        // `section_row_line_red_only_on_a_real_check_failure` below).
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let chip_color = |tone: PrChipTone, checks: Option<crate::workspace::ChecksRollup>| {
            let line = section_row_line(
                false,
                Some("main"),
                None,
                0,
                0,
                Some((7, tone)),
                checks,
                &glyphs,
                &p,
                60,
            );
            let chip = line
                .spans
                .iter()
                .find(|s| s.content.as_ref().contains("PR7"))
                .expect("chip span");
            let has_checks_glyph = line
                .spans
                .iter()
                .any(|s| matches!(s.content.as_ref(), "✓" | "✗" | "●"));
            (chip.style.fg, has_checks_glyph)
        };
        use crate::workspace::ChecksRollup::*;
        assert_eq!(
            chip_color(PrChipTone::Open, Some(Passing)),
            (Some(p.overlay1), true),
            "open + CI green: gray chip, checks glyph kept"
        );
        assert_eq!(
            chip_color(PrChipTone::Open, Some(Failing)),
            (Some(p.overlay1), true),
            "open + CI failing: gray chip, red ✗ carried by the glyph"
        );
        assert_eq!(
            chip_color(PrChipTone::Open, None),
            (Some(p.overlay1), false),
            "open + unknown CI: gray chip, no glyph to color"
        );
        assert_eq!(
            chip_color(PrChipTone::Merged, Some(Failing)),
            (Some(p.overlay1), false),
            "merged: gray chip, stale CI glyph suppressed"
        );
        assert_eq!(
            chip_color(PrChipTone::Draft, None),
            (Some(p.overlay1), false),
            "draft: gray chip"
        );
        assert_eq!(
            chip_color(PrChipTone::Closed, Some(Passing)),
            (Some(p.overlay1), false),
            "closed: gray chip — red is a failing check's alone"
        );
    }

    #[test]
    fn section_row_line_branch_ellipsizes_when_the_cluster_is_wide() {
        // Attribution (T3): was `..._name_wins_in_full_the_branch_
        // ellipsizes` — with the name slot gone there is no name/branch
        // priority left to pin; what survives is the truncation ORDER
        // itself: the cluster is reserved in full, the branch label
        // ellipsizes into the remainder (`spike/m0-ambie…` is the mock's
        // own example), never the reverse.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let branch = "feature/add-a-very-long-descriptive-branch-name";
        let line = section_row_line(
            false,
            Some(branch),
            Some((916, 2)),
            2,
            1,
            None,
            None,
            &glyphs,
            &p,
            45,
        );
        let text = line_text(&line);
        assert!(
            text.contains('…'),
            "a row this tight must truncate something: {text:?}"
        );
        assert!(
            !text.contains(branch),
            "the branch is the one that ellipsizes: {text:?}"
        );
        assert!(
            text.contains("+916 −2"),
            "the cluster never loses a cell to the label: {text:?}"
        );
        assert!(
            display_width(&text) <= 45,
            "row must respect its width budget: {text:?}"
        );
    }

    #[test]
    fn section_row_line_caps_a_huge_ahead_behind_count_display() {
        // Item 2: the owner's real `rails/rails` fork sits ~99485 commits
        // behind. An unbounded integer in the fixed-width state cluster
        // pushes the whole cluster past the row's right edge.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let width = 56;
        let line = section_row_line(
            false,
            Some("main"),
            None,
            1,
            99485,
            None,
            None,
            &glyphs,
            &p,
            width,
        );
        let text = line_text(&line);
        assert!(
            display_width(&text) <= width as usize,
            "the whole row, including the cluster, must fit width: {text:?}"
        );
        assert!(
            text.contains("99+"),
            "a huge count must be capped, not spelled out: {text:?}"
        );
        assert!(
            !text.contains("99485"),
            "the raw huge number must never render: {text:?}"
        );
        assert!(
            text.ends_with(&format!("{}99+", glyphs.behind)),
            "the capped cluster must still be fully present at the right edge: {text:?}"
        );
    }

    #[test]
    fn section_row_line_worktree_marker_reads_as_its_own_element() {
        // Item 3, carried over T3's slot reorder: the marker must read as
        // a marker, not a glyph glued onto the branch (`⌗⎇main`).
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let text = line_text(&section_row_line(
            true,
            Some("muiraquita"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            60,
        ));
        assert!(
            text.contains("⌗ ⎇ muiraquita"),
            "marker, branch glyph and label each keep their own cell: {text:?}"
        );
        assert!(
            !text.contains("⌗\u{2387}"),
            "the marker must never glue onto the branch glyph: {text:?}"
        );
    }

    #[test]
    fn section_row_line_leader_only_exists_with_a_cluster() {
        // Fica vermelho se o leader pontilhado pintar sem cluster, ou
        // deixar de pintar quando há um cluster a alcançar.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let with_cluster =
            section_row_line(false, Some("main"), None, 2, 0, None, None, &glyphs, &p, 60);
        let with_text = line_text(&with_cluster);
        assert!(
            with_text.contains('·'),
            "a cluster gets a dotted leader: {with_text:?}"
        );
        let leader = with_cluster
            .spans
            .iter()
            .find(|s| s.content.contains('·'))
            .expect("leader span");
        assert_eq!(
            leader.style.fg,
            Some(p.surface1),
            "the leader is surface1 — the band ruler's connective colour: {leader:?}"
        );
        let without = line_text(&section_row_line(
            false,
            Some("main"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            60,
        ));
        assert!(
            !without.contains('·'),
            "no cluster, no leader — nothing to lead to: {without:?}"
        );
    }

    #[test]
    fn section_row_line_worktree_marker_is_overlay1_never_mauve() {
        // Fica vermelho se ⌗ aparecer num checkout main, ou se o marcador
        // voltar ao mauve — R1 reserva o mauve para o ProjectRow.
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let line = section_row_line(true, Some("fix/x"), None, 0, 0, None, None, &glyphs, &p, 60);
        let marker = line
            .spans
            .iter()
            .find(|s| s.content.as_ref().contains('⌗'))
            .expect("marker span");
        assert_eq!(
            marker.style.fg,
            Some(p.overlay1),
            "R1: the marker is overlay1, not mauve: {marker:?}"
        );
        assert!(
            !marker.style.add_modifier.contains(Modifier::BOLD),
            "R1: the marker carries no extra emphasis: {marker:?}"
        );
        let main = line_text(&section_row_line(
            false,
            Some("main"),
            None,
            0,
            0,
            None,
            None,
            &glyphs,
            &p,
            60,
        ));
        assert!(
            !main.contains('⌗'),
            "only a linked worktree carries the marker: {main:?}"
        );
    }

    #[test]
    // T7 (bora-79l): the failing glyph span is bare `✗` now (separator
    // spaces are their own spans) — fica vermelho se voltar a carregar o
    // espaço dentro do span.
    fn section_row_line_red_only_on_a_real_check_failure() {
        // Fica vermelho se qualquer coisa além de uma falha real de check
        // pintar de vermelho — behind, diff, ou o chip PR42 (R1).
        let p = Palette::catppuccin();
        let glyphs = unicode_glyphs();
        let failing = section_row_line(
            false,
            Some("main"),
            None,
            0,
            0,
            None,
            Some(crate::workspace::ChecksRollup::Failing),
            &glyphs,
            &p,
            60,
        );
        let failing_glyph = failing
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "✗")
            .expect("failing checks glyph");
        assert_eq!(
            failing_glyph.style.fg,
            Some(p.red),
            "a real failing check is the cluster's one red: {failing_glyph:?}"
        );
        // Everything loud at once — wide diff, far behind, a CLOSED PR —
        // and not a single red cell anywhere.
        let noisy = section_row_line(
            false,
            Some("main"),
            Some((916, 2)),
            0,
            5,
            Some((42, PrChipTone::Closed)),
            None,
            &glyphs,
            &p,
            60,
        );
        assert!(
            noisy.spans.iter().all(|s| s.style.fg != Some(p.red)),
            "behind/diff/PR42 never paint red: {:?}",
            line_text(&noisy)
        );
        let chip = noisy
            .spans
            .iter()
            .find(|s| s.content.as_ref().contains("PR42"))
            .expect("chip span");
        assert_eq!(
            chip.style.fg,
            Some(p.overlay1),
            "the PR chip stays gray even CLOSED: {chip:?}"
        );
    }

    #[test]
    // T7 (bora-79l): collapsed starts ` ▸` after the F gutter — fica
    // vermelho se o caret voltar à coluna 0.
    fn project_row_line_group_header_has_no_chevron_or_ruler() {
        // Item 4 (bora-c1h) established: the approved mock's `.g` rule
        // draws neither a chevron nor a ruler when EXPANDED — the
        // per-workspace `SectionRow` below already owns the `▾`/`▸`
        // disclosure glyph, and Solo #11's dash-fill was a deviation from
        // the approved design. The owner's later ask (item 3b) restores
        // the caret for the CLOSED case only, since a collapsed group
        // shows nothing else beneath it to carry that affordance — the
        // ruler stays gone in both states.
        let p = Palette::catppuccin();
        let expanded = line_text(&project_row_line("CNB", 1, 4, false, &p, 30));
        assert!(
            !expanded.starts_with('▾') && !expanded.starts_with('▸'),
            "an expanded group header draws no chevron: {expanded:?}"
        );
        assert_eq!(
            expanded.chars().filter(|&c| c == '─').count(),
            0,
            "the group header draws no ruler: {expanded:?}"
        );

        let collapsed = line_text(&project_row_line("CNB", 1, 4, true, &p, 30));
        assert!(
            collapsed.starts_with(" ▸"),
            "a closed group header gets its caret back, after the T7 \
             gutter column: {collapsed:?}"
        );
        assert_eq!(
            collapsed.chars().filter(|&c| c == '─').count(),
            0,
            "still no ruler when collapsed: {collapsed:?}"
        );
    }

    #[test]
    fn row_gap_appears_after_a_workspaces_pane_dots_row_only() {
        // Attribution: before this fix a workspace could emit N `PaneRow`s,
        // so the gap only applied after the LAST sibling `PaneRow` of a
        // block. `PaneDotsRow` replaced the whole per-workspace block with
        // exactly ONE 2-line block (bora-79l F2's l1/l2 split). T7
        // (bora-79l, divergence C) then narrowed the gap to BRANCH GROUPS.
        // 6a keeps the rule in the group shape: the LAST member block of
        // a group separates from the next group's header. Fica vermelho
        // se o gap voltar a disparar entre quaisquer dois blocos (ou
        // deixar de disparar entre grupos diferentes): as alturas abaixo
        // mudariam de 3/2 para 2/3.
        let entries = vec![
            WorkspaceListEntry::SectionRow {
                ws_idx: 0,
                checkout_key: "k1".into(),
                collapse_key: "wsec:0".into(),
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "g1".into(),
                diff: None,
            },
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 0,
                name: "ws0".into(),
            },
            WorkspaceListEntry::SectionRow {
                ws_idx: 1,
                checkout_key: "k2".into(),
                collapse_key: "wsec:1".into(),
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "g2".into(),
                diff: None,
            },
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 1,
                name: "ws1".into(),
            },
        ];
        let row_gap = 1;
        assert_eq!(
            entry_row_height(&entries[0], &entries, 0, row_gap),
            1,
            "SectionRow itself never carries the gap"
        );
        assert_eq!(
            entry_row_height(&entries[1], &entries, 1, row_gap),
            3,
            "a PaneDotsRow (base height 2) followed by a DIFFERENT branch \
             group's SectionRow gets +row_gap: {entries:?}"
        );
        assert_eq!(
            entry_row_height(&entries[2], &entries, 2, row_gap),
            1,
            "SectionRow itself never carries the gap"
        );
        assert_eq!(
            entry_row_height(&entries[3], &entries, 3, row_gap),
            2,
            "the LAST PaneDotsRow in the list gets no trailing gap (base height 2 only)"
        );
    }

    #[test]
    fn row_gap_glues_member_blocks_of_one_branch_group() {
        // T7 (bora-79l, divergence C) + 6a: dentro de uma mesma branch os
        // blocos das workspaces membros são contíguos DEBAIXO da header
        // única do grupo — o branco separa apenas o fim de um grupo do
        // próximo header (ALVO_CAPTURE rows 04-07 coladas sob a header
        // `⎇ main`, row 08 em branco). Fica vermelho se membros do mesmo
        // grupo ganharem uma linha em branco entre si, se grupos
        // diferentes colarem, ou se uma header OCULTA (exceção
        // sections-empilhadas) voltar a dobrar o branco.
        let entries = vec![
            WorkspaceListEntry::SectionRow {
                ws_idx: 0,
                checkout_key: "k1".into(),
                collapse_key: "wsec:0".into(),
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "same".into(),
                diff: None,
            },
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 0,
                name: "ws0".into(),
            },
            // 6a: the second member of the SAME group — no SectionRow of
            // its own anymore, just the block glued under the header.
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 1,
                name: "ws1".into(),
            },
            WorkspaceListEntry::SectionRow {
                ws_idx: 2,
                checkout_key: "k3".into(),
                collapse_key: "wsec:2".into(),
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "other".into(),
                diff: None,
            },
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 2,
                name: "ws2".into(),
            },
            // A group whose header is HIDDEN (the stacked-sections
            // same-branch exception): the hidden header's own row is the
            // separator, so the block above it gets no gap — a gap there
            // was the double blank the owner pointed at.
            WorkspaceListEntry::SectionRow {
                ws_idx: 3,
                checkout_key: "k4".into(),
                collapse_key: "wsec:3".into(),
                header_on: true,
                header_hidden: true,
                show_diff: true,
                branch_group: "third".into(),
                diff: None,
            },
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 3,
                name: "ws3".into(),
            },
        ];
        let heights: Vec<u16> = (0..entries.len())
            .map(|idx| entry_row_height(&entries[idx], &entries, idx, 1))
            .collect();
        assert_eq!(
            heights,
            vec![1, 2, 3, 1, 2, 1, 2],
            "member blocks of one group glue (2), the LAST block before a \
             NEW group's VISIBLE header separates (3), and a HIDDEN next \
             header's own row already separates (2): {entries:?}"
        );
    }

    #[test]
    fn pane_dots_columns_are_one_per_pane_spaced_two_apart() {
        let mut ws = Workspace::test_new("ita-principal");
        ws.test_split(Direction::Vertical);
        ws.test_split(Direction::Vertical);
        let width = 30u16;
        let columns = pane_dots_columns(&ws, width);
        assert_eq!(columns.len(), 3, "one dot per pane: {columns:?}");
        for pair in columns.windows(2) {
            assert_eq!(
                pair[1].2 - pair[0].2,
                2,
                "dots sit 2 columns apart (dot + separating space): {columns:?}"
            );
        }
        assert_eq!(
            columns[0].2, PANE_DOTS_INDENT,
            "the first dot starts at the shared block indent: {columns:?}"
        );
    }

    #[test]
    fn pane_dots_dots_line_renders_one_dot_per_pane_and_totals_exact_width() {
        let p = Palette::catppuccin();
        let width = 30u16;
        let dots: Vec<(&'static str, Style)> = vec![("○", Style::default().fg(p.overlay0)); 3];
        let line = pane_dots_dots_line(&dots, width);
        let text = line_text(&line);
        assert_eq!(
            text.matches('○').count(),
            3,
            "one dot glyph per pane: {text:?}"
        );
        assert_eq!(
            display_width(&text),
            width as usize,
            "the row totals exactly width: {text:?}"
        );
        assert!(
            text.starts_with("   "),
            "l2 shares l1's PANE_DOTS_INDENT (3 columns), matching ALVO_CAPTURE \
             rows 05/29: {text:?}"
        );
        assert!(
            text.trim_end().ends_with('○'),
            "the last dot is the row's last non-blank cell — the row pads \
             after them to stay exactly `width`: {text:?}"
        );
    }

    #[test]
    fn pane_dots_name_line_never_contains_a_repo_name() {
        let p = Palette::catppuccin();
        let text = line_text(&pane_dots_name_line("agent-x", &p, 40));

        assert!(text.contains("agent-x"));
        assert!(!text.contains("cnb_landing_page"));
        assert!(!text.contains("bora"));
    }

    #[test]
    fn pane_dots_name_line_uses_column_3_and_overlay1_no_state_glyph() {
        // Gate G1 (bora-79l.3): l1 puts the name at column 3 (indent 3),
        // colors it `overlay1` (the same as `⎇`), and never draws a state
        // glyph — every pane's state is l2's payload.
        let p = Palette::catppuccin();
        let line = pane_dots_name_line("main", &p, 40);
        let text = line_text(&line);
        assert_eq!(
            text.find('m'),
            Some(3),
            "the name starts at column 3: {text:?}"
        );
        let name_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "main")
            .expect("name span");
        assert_eq!(
            name_span.style.fg,
            Some(p.overlay1),
            "l1's name uses overlay1, the same color as the branch glyph: {name_span:?}"
        );
        for glyph in ["⠋", "●", "◆", "○"] {
            assert!(
                !text.contains(glyph),
                "l1 must carry no state glyph at all: {text:?}"
            );
        }
    }

    #[test]
    fn pane_dots_row_is_a_two_line_block_name_then_dots() {
        // G1's own "workspace vira bloco de 2 linhas": entry_row_height's
        // base for `PaneDotsRow` is 2 now, and the render arm draws the
        // name on l1 and the dots on l2 (row_y + 1) — never both on one
        // line, the old single-line row's shape.
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("main");
        ws.test_split(Direction::Vertical);
        app.workspaces = vec![ws];
        app.ensure_test_terminals();
        let pane = app.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = AgentState::Blocked;

        let entries = workspace_list_entries(&app);
        let pane_dots_idx = entries
            .iter()
            .position(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { .. }))
            .expect("Project view must emit a PaneDotsRow for an open workspace");
        assert_eq!(
            entry_row_height(&entries[pane_dots_idx], &entries, pane_dots_idx, 0),
            2,
            "a PaneDotsRow's own content is 2 rows tall"
        );

        let area = Rect::new(0, 0, 30, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal_backend =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal_backend
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");

        let (_cards, _headers, project_rows) = compute_workspace_list_areas_all(&app, area);
        let pane_hit = project_rows
            .iter()
            .find(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
            .expect("one dot hit area for the single pane");
        let l2_y = pane_hit.rect.y;
        let l1_y = l2_y.saturating_sub(1);

        let buffer = terminal_backend.backend().buffer();
        let l1 = row_text(buffer, l1_y, area.width);
        let l2 = row_text(buffer, l2_y, area.width);
        assert!(l1.contains("main"), "l1 carries the name: {l1:?}");
        assert!(
            !l1.contains('◆'),
            "l1 carries no state glyph even for a Blocked pane: {l1:?}"
        );
        assert!(
            !l2.contains("main"),
            "l2 carries no name, only dots: {l2:?}"
        );
        assert!(l2.contains('◆'), "l2 carries the pane's state dot: {l2:?}");
    }

    #[test]
    fn pane_dots_row_hit_areas_land_on_the_rendered_dots_own_columns() {
        // Third lockstep consumer (`pane_dots_columns`'s doc): render and
        // hit-test must derive the SAME dot columns/identities, or a click
        // would silently focus the wrong pane. This proves it end to end —
        // render the real row, then check each hit area's rect against the
        // ACTUAL rendered glyph at that column, not against re-derived
        // arithmetic that could drift the same way twice.
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("ita-principal");
        ws.test_split(Direction::Vertical);
        app.workspaces = vec![ws];

        let area = Rect::new(0, 0, 30, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");

        let (_cards, _headers, project_rows) = compute_workspace_list_areas_all(&app, area);
        let mut pane_hits: Vec<_> = project_rows
            .iter()
            .filter(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
            .collect();
        assert_eq!(pane_hits.len(), 2, "one hit area per pane: {pane_hits:?}");
        pane_hits.sort_by_key(|a| a.rect.x);

        let buffer = terminal.backend().buffer();
        for hit in &pane_hits {
            let cell = &buffer[(hit.rect.x, hit.rect.y)];
            assert_ne!(
                cell.symbol(),
                " ",
                "the hit area at {:?} must land on the rendered dot glyph, not blank space: {cell:?}",
                hit.rect
            );
        }
        // Columns are distinct and 2 cells apart (dot + separating space),
        // matching `pane_dots_columns`'s own arithmetic — proven here
        // against the RENDER's actual output, not by re-deriving the
        // formula a second time.
        assert_eq!(
            pane_hits[1].rect.x - pane_hits[0].rect.x,
            2,
            "dots sit 2 columns apart: {pane_hits:?}"
        );
    }

    #[test]
    fn pane_dots_row_card_rect_is_pinned_to_the_rendered_block_rows() {
        // P2, bora-79l T1: the workspace card's rect is pinned against
        // the RENDERED buffer (same rule as the dot-columns test above),
        // never against re-derived arithmetic that could drift the same
        // way twice. l1 is the row the renderer actually drew the name
        // on, l2 the row the dot glyph lands on, the card covers exactly
        // those two rows, and the branch line above carries no card.
        // Goes red if the card moves off the block (wrong emitting row,
        // height 1, or a SectionRow-card regression).
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("ita-principal");
        ws.test_split(Direction::Vertical);
        app.workspaces = vec![ws];

        let area = Rect::new(0, 0, 30, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");

        let (cards, _headers, project_rows) = compute_workspace_list_areas_all(&app, area);
        let card = cards
            .iter()
            .find(|c| c.ws_idx == 0)
            .expect("the PaneDotsRow block must be the workspace's card");
        let dot_hit = project_rows
            .iter()
            .find(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
            .expect("one dot hit area for the single pane");
        let section_hit = project_rows
            .iter()
            .find(|a| matches!(a.target, ProjectRowTarget::Section { .. }))
            .expect("the SectionRow keeps its own hit area");

        let buffer = terminal.backend().buffer();
        let l1 = (section_hit.rect.y + 1..area.y + area.height)
            .find(|&y| row_text(buffer, y, area.width).contains("ita-principal"))
            .expect("the name line must render below the branch line");
        // The dot hit's own column must land on a rendered glyph (the
        // 9224 test proves this in depth; re-proven here because this
        // test's l2 IS that row).
        assert_ne!(
            buffer[(dot_hit.rect.x, dot_hit.rect.y)].symbol(),
            " ",
            "the dot hit must land on the rendered dot glyph"
        );
        assert_eq!(card.rect.y, l1, "the card starts on the name row");
        assert_eq!(card.rect.height, 2, "the card spans BOTH rows of the block");
        assert_eq!(
            card.rect.y + 1,
            dot_hit.rect.y,
            "the card's second row is the dots row"
        );
        assert!(
            section_hit.rect.y < card.rect.y,
            "the branch line sits above the block and carries no card: \
             section at {:?}, card at {:?}",
            section_hit.rect,
            card.rect
        );
    }

    #[test]
    fn pane_dots_dot_glyph_covers_every_reachable_agent_state() {
        // Gate G1 (bora-79l.3) named the design's "5 estados da 0.45.6";
        // T2 (bora-79l) closed the convergence by splitting `Blocked` on
        // `seen` — attribution, same five combos:
        //
        //   before  Idle+unseen → ● yellow ("esperando você"); no green anywhere
        //   after   Idle+unseen → ● GREEN ("respondeu/pronto · terminou, vem
        //           ler"), Blocked+unseen → ● yellow ("esperando VOCÊ · agent
        //           parou pra perguntar"), Blocked+seen keeps the capture's
        //           `◆ falha real` (ALVO row 05's own combo), spinner moves to
        //           overlay1+BOLD (the alvo mock's `.spin.o1.b`).
        //
        // Every ALVO_CAPTURE text row survives byte for byte (◆ stays ◆,
        // ● stays ●); only hues/bold move. Fica vermelho se qualquer estado
        // trocar de hue (R1: um significado por cor) ou o spinner parar de
        // usar o frame compartilhado.
        let p = Palette::catppuccin();
        let dots = crate::config::StatusIndicatorStyle::Dots;

        let (glyph, style) = pane_dots_dot_glyph(AgentState::Working, true, 0, dots, &p);
        assert_eq!(glyph, "⠋", "Working animates the shared spinner: {glyph:?}");
        assert_eq!(
            style.fg,
            Some(p.overlay1),
            "the spinner is R1 gray — overlay1, the alvo mock's .spin.o1.b"
        );
        assert!(style.add_modifier.contains(Modifier::BOLD));

        let (glyph, style) = pane_dots_dot_glyph(AgentState::Blocked, false, 0, dots, &p);
        assert_eq!(
            glyph, "●",
            "Blocked+unseen (\"esperando VOCÊ · agent parou pra perguntar\") is \
             a STATIC yellow bullet, never an animated glyph: {glyph:?}"
        );
        assert_eq!(style.fg, Some(p.yellow), "R1: amarelo é só esperando VOCÊ");
        assert!(style.add_modifier.contains(Modifier::BOLD));

        let (glyph, style) = pane_dots_dot_glyph(AgentState::Idle, false, 0, dots, &p);
        assert_eq!(
            glyph, "●",
            "Idle+unseen (\"respondeu / pronto · terminou, vem ler\") is a \
             STATIC green bullet: {glyph:?}"
        );
        assert_eq!(style.fg, Some(p.green), "R1: verde é só respondeu/pronto");
        assert!(style.add_modifier.contains(Modifier::BOLD));

        let (glyph, style) = pane_dots_dot_glyph(AgentState::Blocked, true, 0, dots, &p);
        assert_eq!(glyph, "◆", "Blocked+seen (\"falha\") is red: {glyph:?}");
        assert_eq!(style.fg, Some(p.red), "R1: vermelho é só falha real");
        assert!(style.add_modifier.contains(Modifier::BOLD));

        let (glyph, style) = pane_dots_dot_glyph(AgentState::Idle, true, 0, dots, &p);
        assert_eq!(glyph, "○", "Idle+seen (\"parado\"): {glyph:?}");
        assert_eq!(style.fg, Some(p.overlay0));
        assert!(
            !style.add_modifier.contains(Modifier::BOLD),
            "parado is the quiet state — the alvo mock's plain .o0, no .b"
        );

        let (glyph, style) = pane_dots_dot_glyph(AgentState::Unknown, true, 0, dots, &p);
        assert_eq!(
            glyph, "○",
            "Unknown (plain shell) reads as \"parado\", same as Idle+seen: {glyph:?}"
        );
        assert_eq!(style.fg, Some(p.overlay0));

        // Falha reuses the shared dots/symbols preference (`blocked_glyph`),
        // never a second inline match.
        let symbols = crate::config::StatusIndicatorStyle::Symbols;
        let (glyph, _) = pane_dots_dot_glyph(AgentState::Blocked, true, 0, symbols, &p);
        assert_eq!(
            glyph, "×",
            "Symbols preference changes the falha glyph too: {glyph:?}"
        );
    }

    #[test]
    fn pane_dots_row_never_draws_the_pane_row_connector() {
        // The owner's repeated "rabinho" complaint (item 4): `╰ ` was
        // drawn ONLY by `pane_row_line`, called ONLY from the now-dead
        // `PaneRow` arm (grep confirms `╰ ` — the bare, dash-less form —
        // appears nowhere else in this file; `╰── `, 4 cells with dashes,
        // is the unrelated Flat/Repo bracket-rail glyph, never reachable
        // in Project view at all). `pane_dots_row_line` never builds that
        // span, so the connector disappears as a pure consequence of
        // `PaneRow` no longer being emitted here — not because anything
        // strips it out. This renders the real row (not a hand-built
        // `Line`) to prove it end to end, in the spirit of
        // `ui::sidebar::capture`'s instrument (a sibling file this task
        // does not own, so this test builds its own minimal render
        // instead of extending that module).
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("ita-principal");
        ws.test_split(Direction::Vertical);
        app.workspaces = vec![ws];

        let area = Rect::new(0, 0, 30, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        for y in 0..area.height {
            let text = row_text(buffer, y, area.width);
            assert!(
                !text.contains('╰'),
                "no row in Project view may draw the old pane-row connector: {text:?} at row {y}"
            );
        }
    }

    /// Common fixture for the T2 (bora-79l) end-to-end dot tests: one
    /// Project-view workspace whose single root pane sits in the given
    /// `(state, seen)`, already attached to a test terminal.
    fn pane_dots_block_fixture(state: AgentState, seen: bool) -> AppState {
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("alvo-ws");
        ws.test_split(Direction::Vertical);
        app.workspaces = vec![ws];
        app.ensure_test_terminals();
        let pane = app.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = state;
        app.workspaces[0].tabs[0].panes.get_mut(&pane).unwrap().seen = seen;
        app
    }

    #[test]
    fn pane_dots_spinner_frame_advances_between_animation_ticks() {
        // P5 regression lock (T2 contract item 5): the pane dot SPINS while
        // the agent works — the Project-view arm must keep consuming
        // `app.spinner_tick` exactly like the other modes' arms do.
        // Deterministic by construction: the SAME AppState re-rendered with
        // only `spinner_tick` advanced, no wall clock anywhere. Fica vermelho
        // se o braço parar de consumir o tick (a bolinha congelada da
        // regressão P5) — os dois renders seriam byte-idênticos.
        let mut app = pane_dots_block_fixture(AgentState::Working, true);
        let area = Rect::new(0, 0, 30, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let render_dot = |app: &AppState| -> String {
            let mut terminal =
                Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
            terminal
                .draw(|frame| render_workspace_list(app, &runtimes, frame, area, false))
                .expect("workspace list should render");
            let (_cards, _headers, project_rows) = compute_workspace_list_areas_all(app, area);
            let hit = project_rows
                .iter()
                .find(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
                .expect("one dot hit area for the single pane");
            terminal.backend().buffer()[(hit.rect.x, hit.rect.y)]
                .symbol()
                .to_string()
        };

        app.spinner_tick = 0;
        let frame_a = render_dot(&app);
        // Two animation ticks (= 2 × SPINNER_TICK_STEP = 10) provably cross
        // a glyph boundary of `spinner_frame`'s divisor 8; a single tick
        // (5) can land inside the same glyph cell, which is why the
        // deterministic form advances two.
        app.spinner_tick += 2 * crate::app::SPINNER_TICK_STEP;
        let frame_b = render_dot(&app);
        assert_ne!(
            frame_a, frame_b,
            "fica vermelho se o spinner não girar entre ticks: {frame_a:?} == {frame_b:?}"
        );
        assert_ne!(frame_a, " ", "the dot cell holds a glyph, not blank space");
    }

    #[test]
    fn pane_dots_name_stays_overlay1_and_undimmed_in_every_pane_state() {
        // T2 contract item 1: "NUNCA esmaecido/dim — inclusive com painel
        // parado (a parte importante não some)". Renders the real block
        // under every reachable (state, seen) combo and pins the name
        // cells' fg AND the absence of DIM on the RENDERED buffer — not on
        // a hand-built Line. Fica vermelho se qualquer estado voltar a
        // esmaecer o nome (o comportamento da linha única antiga) ou trocar
        // a cor do nome por outra que não o overlay1 do rótulo de branch.
        let p = Palette::catppuccin();
        for (state, seen) in [
            (AgentState::Working, true),
            (AgentState::Blocked, false),
            (AgentState::Blocked, true),
            (AgentState::Idle, false),
            (AgentState::Idle, true),
            (AgentState::Unknown, true),
        ] {
            let app = pane_dots_block_fixture(state, seen);
            let area = Rect::new(0, 0, 30, 10);
            let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
            let mut terminal =
                Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
            terminal
                .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
                .expect("workspace list should render");
            let (_cards, _headers, project_rows) = compute_workspace_list_areas_all(&app, area);
            let hit = project_rows
                .iter()
                .find(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
                .expect("one dot hit area for the single pane");
            let l1_y = hit.rect.y.saturating_sub(1);

            let buffer = terminal.backend().buffer();
            let l1 = row_text(buffer, l1_y, area.width);
            let name_col = l1
                .find(|c: char| !c.is_whitespace())
                .unwrap_or_else(|| panic!("l1 must carry the name: {l1:?}"));
            let name_len = "alvo-ws".len();
            for x in name_col..name_col + name_len {
                let cell = &buffer[(x as u16, l1_y)];
                assert_eq!(
                    cell.fg, p.overlay1,
                    "fica vermelho se o nome deixar de ser overlay1 em \
                     {state:?}+seen={seen} (col {x}): {l1:?}"
                );
                assert!(
                    !cell.modifier.contains(Modifier::DIM),
                    "fica vermelho se o nome esmaecer em {state:?}+seen={seen}: {l1:?}"
                );
            }
        }
    }

    #[test]
    fn pane_dots_dots_start_at_the_rendered_name_column() {
        // T2 contract item 2: as bolinhas ficam na MESMA coluna do nome
        // (col 3), na l2 — anchored against the RENDERED buffer, not
        // against re-derived arithmetic that could drift the same way
        // twice: the first non-blank column of l2 must be EXACTLY the
        // first non-blank column of l1, and the dot hit-rect must land on
        // that same column. Fica vermelho se as bolinhas deslocarem da
        // coluna do nome (ex.: ancorarem na largura renderizada do nome ou
        // no canto direito da linha).
        let app = pane_dots_block_fixture(AgentState::Blocked, true);
        let area = Rect::new(0, 0, 30, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");
        let (_cards, _headers, project_rows) = compute_workspace_list_areas_all(&app, area);
        let hit = project_rows
            .iter()
            .find(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
            .expect("one dot hit area for the single pane");

        let buffer = terminal.backend().buffer();
        let l1 = row_text(buffer, hit.rect.y.saturating_sub(1), area.width);
        let l2 = row_text(buffer, hit.rect.y, area.width);
        let name_col = l1
            .find(|c: char| !c.is_whitespace())
            .expect("l1 carries the name");
        let dot_col = l2
            .find(|c: char| !c.is_whitespace())
            .expect("l2 carries the dot glyph");
        assert_eq!(
            dot_col, name_col,
            "fica vermelho se a bolinha sair da coluna do nome: l1 {l1:?} vs l2 {l2:?}"
        );
        assert_eq!(
            hit.rect.x, dot_col as u16,
            "the hit-rect lands on that same rendered column: {l2:?}"
        );
    }

    #[test]
    fn pane_dots_name_line_carries_no_diff_even_with_a_change_set() {
        // T7 (bora-79l, divergence A) killed T2's contract item 1 by
        // assignment: nenhuma PaneDotsRow l1 carrega `+N −M` — o diff vive
        // só no cluster da header (ALVO_CAPTURE row 27 vs row 28: a header
        // carrega `+916 −2`, a l1 é `hotfix` puro). Fica vermelho se o diff
        // voltar à l1 (a render arm voltando a passá-lo ou o builder o
        // aceitando de novo).
        let p = Palette::catppuccin();
        let text = line_text(&pane_dots_name_line("hotfix", &p, 40));
        assert_eq!(
            text.trim_end(),
            "   hotfix",
            "l1 is exactly the indent + the name, nothing beside it: {text:?}"
        );

        // End to end: a workspace WITH a cached change set still renders a
        // bare l1 — the numbers belong to the SectionRow header's cluster.
        let mut app = pane_dots_block_fixture(AgentState::Idle, true);
        app.workspaces[0].cached_change_set = Some(crate::workspace::WorkspaceChangeSet {
            sections: vec![crate::workspace::ChangeSection {
                kind: crate::workspace::ChangeSectionKind::Unstaged,
                files: vec![crate::workspace::ChangedFile {
                    path: "src/sidebar.rs".to_string(),
                    added: Some(916),
                    removed: Some(2),
                    status: crate::workspace::ChangeStatus::Modified,
                }],
            }],
            base_ref: None,
        });
        let area = Rect::new(0, 0, 40, 10);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");
        let (_cards, _headers, project_rows) = compute_workspace_list_areas_all(&app, area);
        let hit = project_rows
            .iter()
            .find(|a| matches!(a.target, ProjectRowTarget::Pane { .. }))
            .expect("one dot hit area for the single pane");
        let buffer = terminal.backend().buffer();
        let l1 = row_text(buffer, hit.rect.y.saturating_sub(1), area.width);
        assert!(
            l1.contains("alvo-ws") && !l1.contains("+916"),
            "fica vermelho se a l1 renderizada carregar o diff: {l1:?}"
        );
    }

    #[test]
    fn project_view_geometry_emits_one_hit_area_per_row_with_correct_targets() {
        let entries = vec![
            WorkspaceListEntry::ProjectRow {
                name: "cnb".into(),
                collapse_key: "proj:cnb".into(),
                live: 1,
                total: 2,
                declared: true,
            },
            WorkspaceListEntry::SectionRow {
                ws_idx: 0,
                checkout_key: "checkout:1".into(),
                collapse_key: "wsec:0".into(),
                diff: None,
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "g".into(),
            },
            WorkspaceListEntry::SectionHeader {
                name: None,
                kind: &COMMANDS,
                collapse_key: "sec:1".into(),
                done: 1,
                total: 3,
            },
            WorkspaceListEntry::SectionItem {
                kind: &COMMANDS,
                label: "dev".into(),
                detail: Some(":5173".into()),
                running: true,
                ws_idx: Some(0),
            },
        ];
        let body = Rect::new(0, 0, 30, 20);
        let app = AppState::test_new();

        let (cards, headers, project_rows) =
            workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        // Attribution (P2, bora-79l T1): this asserted first
        // `cards.is_empty()` (no row emitted a card), then one card on the
        // `SectionRow`. The card's owner moved again — onto the
        // `PaneDotsRow` block — so a `SectionRow`-only fixture is back to
        // no cards: this fixture has no `PaneDotsRow`, and the branch line
        // must NOT emit a card anymore. `section_row_pushes…`'s successor
        // below pins the block-side card.
        assert!(
            cards.is_empty(),
            "SectionRow must not push a workspace card — the PaneDotsRow \
             block owns it now (P2): {cards:?}"
        );
        assert!(
            headers.is_empty(),
            "Project-view rows are not group headers"
        );
        assert_eq!(project_rows.len(), entries.len());
        for (i, area) in project_rows.iter().enumerate() {
            assert_eq!(area.rect.height, 1, "every Project-view row is height 1");
            assert_eq!(
                area.rect.y,
                body.y + i as u16,
                "rows must not overlap or skip"
            );
        }
        assert_eq!(
            project_rows[0].target,
            ProjectRowTarget::Project {
                collapse_key: "proj:cnb".into()
            }
        );
        assert_eq!(
            project_rows[1].target,
            ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: "checkout:1".into(),
                collapse_key: "wsec:0".into(),
            }
        );
        assert_eq!(
            project_rows[2].target,
            ProjectRowTarget::Band {
                collapse_key: "sec:1".into()
            }
        );
        assert_eq!(
            project_rows[3].target,
            ProjectRowTarget::SectionItem {
                kind: &COMMANDS,
                label: "dev".into(),
                ws_idx: Some(0),
            }
        );
    }

    #[test]
    fn project_view_geometry_unopened_worktree_targets_open_worktree() {
        let entries = vec![WorkspaceListEntry::WorktreeRow {
            checkout_key: "checkout:2".into(),
            repo: Some("cnb_hono".into()),
            branch: "main".into(),
            ahead: 0,
            behind: 0,
            pr: None,
            collapse_key: "wt:2".into(),
            unopened: true,
        }];
        let app = AppState::test_new();
        let (_, _, project_rows) =
            workspace_list_areas_for_entries(&entries, &app, 0, Rect::new(0, 0, 30, 10), 0);

        assert_eq!(
            project_rows[0].target,
            ProjectRowTarget::OpenWorktree {
                checkout_key: "checkout:2".into()
            }
        );
    }

    #[test]
    fn pr_row_height_is_one() {
        let entry = WorkspaceListEntry::PrRow {
            number: 1,
            title: "t".into(),
            url: "u".into(),
            head_ref: "h".into(),
            is_draft: false,
            checks: None,
            ws_idx: None,
        };
        assert_eq!(entry_row_height(&entry, &[], 0, 0), 1);
    }

    #[test]
    fn pr_checks_glyph_matches_rollup_and_reuses_the_checks_palette() {
        let p = Palette::catppuccin();
        assert!(
            pr_checks_glyph(None, &p).is_none(),
            "None rollup shows no trailing glyph"
        );
        let (glyph, style) = pr_checks_glyph(Some(crate::workspace::ChecksRollup::Passing), &p)
            .expect("Passing must show a glyph");
        assert_eq!(glyph, " ✓");
        assert_eq!(style.fg, Some(p.green));
        let (glyph, style) = pr_checks_glyph(Some(crate::workspace::ChecksRollup::Failing), &p)
            .expect("Failing must show a glyph");
        assert_eq!(glyph, " ✗");
        assert_eq!(style.fg, Some(p.red));
        let (glyph, style) = pr_checks_glyph(Some(crate::workspace::ChecksRollup::Pending), &p)
            .expect("Pending must show a glyph");
        assert_eq!(glyph, " ●");
        assert_eq!(style.fg, Some(p.yellow));
    }

    #[test]
    fn pr_row_line_marks_draft_and_shows_number_title_and_checks_glyph() {
        let p = Palette::catppuccin();
        let draft = line_text(&pr_row_line(
            12,
            "wip: thing",
            true,
            Some(crate::workspace::ChecksRollup::Pending),
            &p,
            40,
        ));
        assert!(draft.contains("#12"), "{draft:?}");
        assert!(draft.contains("wip: thing"), "{draft:?}");
        let live = line_text(&pr_row_line(
            13,
            "ready: thing",
            false,
            Some(crate::workspace::ChecksRollup::Failing),
            &p,
            40,
        ));
        assert!(live.contains("#13"), "{live:?}");
        assert!(live.contains("ready: thing"), "{live:?}");
        assert!(live.contains('✗'), "{live:?}");
    }

    #[test]
    fn pane_dots_row_block_is_the_workspace_card_and_the_branch_line_is_not() {
        // P2, bora-79l T1 — successor of
        // `section_row_pushes_workspace_card_area_matching_its_hit_area`
        // (same fixture, attribution flip): the `WorkspaceCardArea` — what
        // right-click, drag-reorder, press and selection painting all key
        // off — moved from the branch line to the workspace's own 2-row
        // block. Goes red if the card is deleted from the `PaneDotsRow`
        // arm, if its rect stops covering BOTH rows, or if the `SectionRow`
        // arm regresses to pushing its own card again.
        let entries = vec![
            WorkspaceListEntry::SectionRow {
                ws_idx: 5,
                checkout_key: "checkout:5".into(),
                collapse_key: "wsec:5".into(),
                diff: None,
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "g".into(),
            },
            WorkspaceListEntry::PaneDotsRow {
                dots: true,
                ws_idx: 5,
                name: "agent".into(),
            },
        ];
        let body = Rect::new(0, 0, 30, 20);
        let app = AppState::test_new();

        let (cards, _headers, project_rows) =
            workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        let section_hit = project_rows
            .iter()
            .find(|a| matches!(&a.target, ProjectRowTarget::Section { ws_idx, .. } if *ws_idx == 5))
            .expect("SectionRow must still get its ProjectRowHitArea");
        assert_eq!(
            cards,
            vec![crate::app::state::WorkspaceCardArea {
                ws_idx: 5,
                rect: Rect::new(section_hit.rect.x, section_hit.rect.y + 1, body.width, 2),
                indented: true,
            }],
            "exactly one card, covering the PaneDotsRow's TWO rows (l1 name \
             + l2 dots) at full body width — never the branch line above"
        );
    }

    #[test]
    fn pane_dots_row_block_paints_selection_on_both_rows_and_the_active_bar_at_the_border() {
        // P2, bora-79l T1 — successor of
        // `section_row_paints_selection_and_active_backgrounds` (same
        // fixture, attribution flip): the selection fill and the active
        // bar live on the workspace's own 2-row block now. Goes red if the
        // `PaneDotsRow` render arm stops filling BOTH rows on selection,
        // stops drawing the active bar, or the `SectionRow` arm regresses
        // to painting the branch line.
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        app.workspaces = vec![Workspace::test_new("alpha"), Workspace::test_new("beta")];
        app.active = Some(0);
        app.selected = 1;
        app.mode = Mode::Navigate;

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 30, 20);
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, true))
            .expect("workspace list should render");

        let (cards, _, _) = compute_workspace_list_areas_all(&app, area);
        let active_card = cards
            .iter()
            .find(|c| c.ws_idx == 0)
            .expect("the active workspace's PaneDotsRow block must push a card");
        let selected_card = cards
            .iter()
            .find(|c| c.ws_idx == 1)
            .expect("the cursored workspace's PaneDotsRow block must push a card");

        let buffer = terminal.backend().buffer();
        // Selection fills BOTH rows of the block — l1 and l2, no hole.
        for y in [selected_card.rect.y, selected_card.rect.y + 1] {
            assert_eq!(
                buffer[(selected_card.rect.x, y)].bg,
                workspace_selection_background(&app.palette, false),
                "the navigate-mode cursor must fill row {y} of the block (l1 AND l2)"
            );
        }
        // The active (but not cursored) workspace gets the blue bar at the
        // block's left border on both rows instead of a fill.
        for y in [active_card.rect.y, active_card.rect.y + 1] {
            assert_eq!(
                buffer[(active_card.rect.x, y)].symbol(),
                "▎",
                "the active block's left border carries the bar on row {y}"
            );
            assert_eq!(
                buffer[(active_card.rect.x, y)].fg,
                app.palette.accent,
                "the bar is the accent colour on row {y}"
            );
        }
        // The branch line above the cursored block stays unpainted — the
        // workspace affordances left it (decision 7).
        let branch_row = selected_card.rect.y.saturating_sub(1);
        assert_eq!(
            buffer[(selected_card.rect.x, branch_row)].bg,
            Color::Reset,
            "the SectionRow branch line must not paint the workspace selection anymore"
        );
    }

    #[test]
    fn project_row_background_is_slightly_lighter_than_sidebar_bg() {
        // Item 3c: the project header row now fills its whole width with
        // `p.surface0`, a lightness step up from the (typically `Reset`)
        // sidebar background — visual weight that replaces the BOLD
        // dropped from the name span (item 6).
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        app.workspaces = vec![Workspace::test_new("alpha")];

        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let area = Rect::new(0, 0, 30, 10);
        let mut terminal =
            Terminal::new(TestBackend::new(area.width, area.height)).expect("test terminal");
        terminal
            .draw(|frame| render_workspace_list(&app, &runtimes, frame, area, false))
            .expect("workspace list should render");

        let (_, _, project_rows) = compute_workspace_list_areas_all(&app, area);
        let project_row = project_rows
            .iter()
            .find(|a| matches!(&a.target, ProjectRowTarget::Project { .. }))
            .expect("an implicit ProjectRow must render for orphan workspaces");
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(project_row.rect.x, project_row.rect.y)].bg,
            app.palette.surface0,
            "the project header row fills with surface0: {:?}",
            buffer[(project_row.rect.x, project_row.rect.y)]
        );
    }

    #[test]
    fn project_view_geometry_pr_row_without_ws_idx_gets_no_hit_area_but_advances_row_y() {
        // The PrRow must not desync the geometry pass for rows after it. A
        // row whose repo has no open workspace carries `ws_idx: None` and
        // stays un-clickable — originally because no `ProjectRowTarget`
        // variant existed at all, now because there is nothing to name as
        // the worktree's repo. Either way its row_y span still counts.
        let entries = vec![
            WorkspaceListEntry::SectionHeader {
                name: None,
                kind: &PULL_REQUESTS,
                collapse_key: "sec:prs:proj".into(),
                done: 0,
                total: 1,
            },
            WorkspaceListEntry::PrRow {
                number: 42,
                title: "fix thing".into(),
                url: "https://github.com/owner/repo/pull/42".into(),
                head_ref: "fix/thing".into(),
                is_draft: false,
                checks: Some(crate::workspace::ChecksRollup::Passing),
                ws_idx: None,
            },
            WorkspaceListEntry::SectionRow {
                ws_idx: 0,
                checkout_key: "checkout:1".into(),
                collapse_key: "wsec:0".into(),
                diff: None,
                header_on: true,
                header_hidden: false,
                show_diff: true,
                branch_group: "g".into(),
            },
        ];
        let body = Rect::new(0, 0, 30, 20);
        let app = AppState::test_new();

        let (cards, headers, project_rows) =
            workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        // Attribution (P2, bora-79l T1): the SectionRow here used to push
        // the one card — its ownership moved to the `PaneDotsRow` block,
        // which this fixture doesn't emit, so cards are empty again. The
        // point of the test is unchanged: the ws_idx-less PrRow advances
        // `row_y` (SectionRow lands 2 rows down) without ever producing a
        // hit area of its own.
        assert!(
            cards.is_empty(),
            "no card: the PrRow has no ws_idx and the SectionRow no longer \
             owns one (P2): {cards:?}"
        );
        assert!(headers.is_empty());
        assert_eq!(
            project_rows.len(),
            2,
            "the SectionHeader and SectionRow get hit areas, the ws_idx-less PrRow does not: {project_rows:?}"
        );
        assert_eq!(project_rows[0].rect.y, body.y, "SectionHeader at row 0");
        assert_eq!(
            project_rows[1].rect.y,
            body.y + 2,
            "SectionRow must land 2 rows down — the PrRow's own row_y span \
             still counted even though it produced no hit area"
        );
        assert_eq!(
            project_rows[1].target,
            ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: "checkout:1".into(),
                collapse_key: "wsec:0".into(),
            }
        );
    }

    #[test]
    fn project_view_geometry_pr_row_with_ws_idx_targets_open_pr() {
        // The clickable half: a PR row whose repo has an open workspace gets
        // an `OpenPr` hit area carrying that workspace and the PR number —
        // the pair `request_open_pr_worktree` consumes. Without this the row
        // would render and silently do nothing, which is worse than no row.
        let entries = vec![WorkspaceListEntry::PrRow {
            number: 42,
            title: "fix thing".into(),
            url: "https://github.com/owner/repo/pull/42".into(),
            head_ref: "fix/thing".into(),
            is_draft: false,
            checks: None,
            ws_idx: Some(3),
        }];
        let body = Rect::new(0, 0, 30, 20);
        let app = AppState::test_new();

        let (_, _, project_rows) = workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        assert_eq!(project_rows.len(), 1, "{project_rows:?}");
        assert_eq!(project_rows[0].rect.y, body.y);
        assert_eq!(
            project_rows[0].target,
            ProjectRowTarget::OpenPr {
                ws_idx: 3,
                number: 42
            }
        );
    }

    #[test]
    fn section_row_emits_plus_area_before_the_section_area_keyed_by_branch_group() {
        // T4 (bora-79l, P3): a visible SectionRow emits a 3-cell
        // `SectionNew` hit area at the row's trailing edge, carrying the
        // section's (repo_identity, branch) — the branch-group pair, never
        // a ws_idx (T6 re-keys nothing). Fica vermelho se:
        // - a emissão sumir (Project view fica sem +, o wiring inexistente
        //   de novo);
        // - a área vier DEPOIS da Section (project_row_target_at pega a
        //   primeira — o clique no + cairia no toggle de collapse);
        // - o par vier trocado/ausente (a criação miraria outro repo).
        let entries = vec![WorkspaceListEntry::SectionRow {
            ws_idx: 0,
            checkout_key: "checkout:1".into(),
            collapse_key: "wsec:0".into(),
            diff: None,
            header_on: true,
            header_hidden: false,
            show_diff: true,
            branch_group: "g".into(),
        }];
        let body = Rect::new(2, 5, 30, 20);
        let mut app = AppState::test_new();
        app.workspaces = vec![git_space_member_on_branch("proj", "key-p", false, "main")];

        let (_, _, project_rows) = workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        assert_eq!(
            project_rows.len(),
            2,
            "the + and the full-row Section area, nothing else: {project_rows:?}"
        );
        assert_eq!(
            project_rows[0].target,
            ProjectRowTarget::SectionNew {
                repo_identity: "key-p".into(),
                branch: "main".into(),
            },
            "the + comes FIRST so first-match hit-testing wins inside its cells"
        );
        assert_eq!(
            project_rows[0].rect,
            Rect::new(body.x + body.width - 3, body.y, 3, 1),
            "same 3-cell trailing-edge convention as the Flat/Repo headers"
        );
        assert_eq!(
            project_rows[1].target,
            ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: "checkout:1".into(),
                collapse_key: "wsec:0".into(),
            },
            "the full-row Section area is still emitted — moving areas never \
             drops one (AGENTS.md binding rule)"
        );
    }

    #[test]
    fn section_row_hidden_header_emits_no_plus_area() {
        // A hidden header paints nothing (T3's same-branch exception / model
        // switch) — a + on an invisible row would be a dead glyph AND a
        // dead click. Fica vermelho se o + passar a pintar/emitir em rows
        // ocultas.
        let entries = vec![WorkspaceListEntry::SectionRow {
            ws_idx: 0,
            checkout_key: "checkout:1".into(),
            collapse_key: "wsec:0".into(),
            diff: None,
            header_on: true,
            header_hidden: true,
            show_diff: true,
            branch_group: "g".into(),
        }];
        let body = Rect::new(0, 0, 30, 20);
        let mut app = AppState::test_new();
        app.workspaces = vec![git_space_member_on_branch("proj", "key-p", false, "main")];

        let (_, _, project_rows) = workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        assert!(
            project_rows.is_empty(),
            "hidden header: no Section, no + — the row only advances row_y: {project_rows:?}"
        );
    }

    #[test]
    fn section_row_without_git_identity_emits_no_plus_area() {
        // No git space (or no branch) → there is no repo to create a
        // worktree in: the Section area survives untouched, the + does not
        // render as a dead affordance. Fica vermelho se o + começar a
        // existir pra sections sem repo.
        let entries = vec![WorkspaceListEntry::SectionRow {
            ws_idx: 0,
            checkout_key: "checkout:1".into(),
            collapse_key: "wsec:0".into(),
            diff: None,
            header_on: true,
            header_hidden: false,
            show_diff: true,
            branch_group: "ws-no-space:x".into(),
        }];
        let body = Rect::new(0, 0, 30, 20);
        let mut app = AppState::test_new();
        app.workspaces = vec![Workspace::test_new("plain")];

        let (_, _, project_rows) = workspace_list_areas_for_entries(&entries, &app, 0, body, 0);

        assert_eq!(project_rows.len(), 1, "{project_rows:?}");
        assert!(
            matches!(project_rows[0].target, ProjectRowTarget::Section { .. }),
            "Section stays; only the + is withheld"
        );
    }

    #[test]
    fn section_row_plus_paints_under_mouse_capture_and_reserves_cluster_budget() {
        // T4 (bora-79l, P3): with mouse capture the SectionRow paints the
        // Flat/Repo-convention " + " at its trailing edge; the cluster's
        // budget shrinks by those 3 cells instead of being overwritten
        // (T7 divergence B's flush-right cluster stays intact, just pinned
        // 3 cells earlier). Without capture the row renders exactly as
        // before — no glyph, full-width cluster. Fica vermelho se o +
        // pintar sem captura, sumir com ela, ou comer o cluster.
        let render = |mouse_capture: bool| -> String {
            let mut app = AppState::test_new();
            app.view_mode = crate::config::ViewMode::Project;
            app.mouse_capture = mouse_capture;
            app.workspaces = vec![git_space_member_on_branch("proj", "key-p", false, "main")];
            app.active = Some(0);
            app.mode = Mode::Terminal;
            let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
            let mut terminal = Terminal::new(TestBackend::new(30, 8)).expect("test terminal");
            terminal
                .draw(|frame| {
                    render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 30, 8), false)
                })
                .expect("workspace list should render");
            let buffer = terminal.backend().buffer();
            (0..8)
                .map(|y| row_text(buffer, y, 30))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let with_plus = render(true);
        assert!(
            with_plus.contains('\u{2387}'),
            "the section header renders: {with_plus:?}"
        );
        let header_row = with_plus
            .lines()
            .find(|line| line.contains('\u{2387}'))
            .expect("header row");
        assert!(
            header_row.trim_end().ends_with('+'),
            "the + rides the trailing edge, Flat/Repo convention: {header_row:?}"
        );

        let without_plus = render(false);
        assert!(
            !without_plus.contains(" + "),
            "no mouse capture, no glyph: {without_plus:?}"
        );
        let bare_header = without_plus
            .lines()
            .find(|line| line.contains('\u{2387}'))
            .expect("header row");
        assert!(
            !bare_header.trim_end().ends_with('+'),
            "capture-off renders the row exactly as before (the P4-A fixture \
             shape — cluster-less rows stay as short as their content): \
             {bare_header:?}"
        );
    }
}
