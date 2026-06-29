use std::time::Instant;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::scrollbar::{render_scrollbar, should_show_scrollbar};
use super::status::{agent_icon, state_dot, state_label, state_label_color};
use crate::app::state::{AgentPanelSort, Palette};
use crate::app::{AppState, Mode};
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

const WORKSPACE_SECTION_HEADER_ROWS: u16 = 2;
const AGENT_PANEL_HEADER_ROWS: u16 = 3;

pub(crate) struct AgentPanelEntry {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub primary_label: String,
    pub primary_tab_label: Option<String>,
    pub agent_label: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub idle_since: Option<std::time::Instant>,
    pub last_agent_state_change_seq: Option<u64>,
    pub custom_status: Option<String>,
    pub state_labels: std::collections::HashMap<String, String>,
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
    if area.width == 0 || area.height < 2 {
        return Rect::default();
    }

    let label = agent_panel_sort_label(sort);
    let width = label.chars().count() as u16;
    Rect::new(
        area.x + area.width.saturating_sub(width),
        area.y + 1,
        width,
        1,
    )
}

pub(crate) fn agent_panel_entries(app: &AppState) -> Vec<AgentPanelEntry> {
    agent_panel_entries_with_runtimes(app, None)
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
    let empty_runtimes;
    let terminal_runtimes = match terminal_runtimes {
        Some(terminal_runtimes) => terminal_runtimes,
        None => {
            empty_runtimes = TerminalRuntimeRegistry::new();
            &empty_runtimes
        }
    };

    let mut entries: Vec<_> = app
        .workspaces
        .iter()
        .enumerate()
        .flat_map(|(ws_idx, ws)| {
            let multi_tab = ws.tabs.len() > 1;
            let workspace_label = ws.display_name_from(&app.terminals, terminal_runtimes);
            ws.pane_details(&app.terminals)
                .into_iter()
                .map(move |detail| AgentPanelEntry {
                    ws_idx,
                    tab_idx: detail.tab_idx,
                    pane_id: detail.pane_id,
                    primary_label: workspace_label.clone(),
                    primary_tab_label: multi_tab.then_some(detail.tab_label),
                    agent_label: Some(detail.agent_label),
                    state: detail.state,
                    seen: detail.seen,
                    idle_since: detail.idle_since,
                    last_agent_state_change_seq: detail.last_agent_state_change_seq,
                    custom_status: detail.custom_status,
                    state_labels: detail.state_labels,
                })
        })
        .collect();

    if matches!(app.agent_panel_sort, AgentPanelSort::Priority) {
        entries.sort_by_key(|entry| {
            (
                std::cmp::Reverse(workspace_attention_priority(entry.state, entry.seen)),
                std::cmp::Reverse(entry.last_agent_state_change_seq),
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

fn truncate_text(text: &str, max_width: usize) -> String {
    let len = text.chars().count();
    if len <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let prefix: String = text.chars().take(max_width.saturating_sub(1)).collect();
    format!("{prefix}…")
}

fn format_agent_panel_primary_label(entry: &AgentPanelEntry, max_width: usize) -> String {
    let Some(tab_label) = entry.primary_tab_label.as_deref() else {
        return truncate_text(&entry.primary_label, max_width);
    };

    let separator = " · ";
    let separator_width = separator.chars().count();
    if max_width <= separator_width + 2 {
        return truncate_text(
            &format!("{}{}{}", entry.primary_label, separator, tab_label),
            max_width,
        );
    }

    let available = max_width.saturating_sub(separator_width);
    let min_tab = 4.min(available.saturating_sub(1)).max(1);
    let preferred_workspace = ((available * 2) / 3).max(1);
    let mut workspace_budget = preferred_workspace
        .min(available.saturating_sub(min_tab))
        .max(1);
    let mut tab_budget = available.saturating_sub(workspace_budget);

    let workspace_len = entry.primary_label.chars().count();
    let tab_len = tab_label.chars().count();

    if workspace_len < workspace_budget {
        let spare = workspace_budget - workspace_len;
        workspace_budget = workspace_len;
        tab_budget = (tab_budget + spare).min(available.saturating_sub(workspace_budget));
    }
    if tab_len < tab_budget {
        let spare = tab_budget - tab_len;
        tab_budget = tab_len;
        workspace_budget = (workspace_budget + spare).min(available.saturating_sub(tab_budget));
    }

    format!(
        "{}{}{}",
        truncate_text(&entry.primary_label, workspace_budget),
        separator,
        truncate_text(tab_label, tab_budget)
    )
}

/// Tree rail for a workspace listed under a branch header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchRail {
    /// Loose workspace with no detected branch — no tree spine.
    None,
    /// Under a branch; the project spine continues down to the closer (│).
    Spine,
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
                .filter(|d| d.tab_idx == t && !d.seen && d.state == AgentState::Idle)
                .filter_map(|d| d.idle_since)
                .map(|since| now.saturating_duration_since(since))
                .max()
        })
        .collect()
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
    /// Repo/project header (no chevron); indented when nested under a visual group.
    ProjectHeader {
        name: String,
        collapse_key: String,
        indented: bool,
    },
    /// Non-clickable branch sub-header inside a project group (╭ label ↑a ↓b).
    BranchHeader {
        label: String,
        ahead: usize,
        behind: usize,
        indented: bool,
    },
    /// Closing line for a project's branch sub-tree (╰───).
    ProjectFooter { indented: bool },
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
        WorkspaceListEntry::ProjectFooter { .. } => 1,
        WorkspaceListEntry::Workspace { .. } => 2,
    }
}

pub(crate) fn normalized_workspace_scroll(app: &AppState, area: Rect, requested: usize) -> usize {
    let ws_area = workspace_list_rect(area, app.sidebar_section_split);
    let body = workspace_list_body_rect(ws_area, false);
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
    // --- Worktree group setup ---
    let mut members_by_key = std::collections::HashMap::<String, Vec<usize>>::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        if let Some(space) = ws.git_space() {
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
        if let Some(ref group_name) = ws.visual_group {
            visual_group_members
                .entry(group_name.clone())
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
        if ws.visual_group.is_some() {
            if let Some(space) = ws
                .git_space()
                .filter(|s| grouped_keys.contains(&s.repo_identity) && !s.is_linked_worktree)
            {
                if let Some(members) = members_by_key.get(&space.repo_identity) {
                    for &m in members {
                        if m != ws_idx
                            && app
                                .workspaces
                                .get(m)
                                .is_some_and(|w| w.visual_group.is_none())
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

    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        if consumed.contains(&ws_idx) {
            continue;
        }

        let in_worktree_group = ws
            .git_space()
            .filter(|space| grouped_keys.contains(&space.repo_identity))
            .is_some();

        if in_worktree_group && !in_visual_group.contains(&ws_idx) {
            let space = ws.git_space().unwrap();
            if emitted_worktree_groups.contains(&space.repo_identity) {
                continue;
            }
            emitted_worktree_groups.insert(space.repo_identity.clone());

            let Some(members) = members_by_key.get(&space.repo_identity) else {
                continue;
            };
            // Always synthesize a project header (the repo label); every checkout
            // of the repo becomes a member inside a branch bracket beneath it.
            let collapsed = !force_expanded && app.collapsed_space_keys.contains(&space.repo_identity);
            entries.push(WorkspaceListEntry::ProjectHeader {
                name: space.label.clone(),
                collapse_key: space.repo_identity.clone(),
                indented: false,
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
                emit_branch_subgroups(app, members, true, &mut entries);
            }
            continue;
        }

        if in_worktree_group {
            let space = ws.git_space().unwrap();
            if emitted_worktree_groups.contains(&space.repo_identity) {
                continue;
            }
        }

        // --- Visual group handling ---
        if in_visual_group.contains(&ws_idx) {
            let group_name = ws
                .visual_group
                .as_deref()
                .expect("in_visual_group only set for workspaces with visual_group");
            if emitted_visual_groups.insert(group_name.to_owned()) {
                let vg_key = format!("vg:{group_name}");
                let collapsed = !force_expanded && app.collapsed_space_keys.contains(&vg_key);
                entries.push(WorkspaceListEntry::GroupHeader {
                    name: group_name.to_owned(),
                    collapse_key: vg_key,
                });
                if !collapsed {
                    if let Some(vg_members) = visual_group_members.get(group_name) {
                        for &member_idx in vg_members {
                            let member_ws = &app.workspaces[member_idx];
                            let repo = member_ws
                                .git_space()
                                .filter(|s| grouped_keys.contains(&s.repo_identity))
                                .map(|s| (s.repo_identity.clone(), s.label.clone()));

                            if let Some((repo_id, label)) = repo {
                                // One synthesized project header per repo group; skip
                                // members whose group was already emitted (clones/worktrees).
                                if !emitted_worktree_groups.insert(repo_id.clone()) {
                                    continue;
                                }
                                let wt_collapsed = !force_expanded && app.collapsed_space_keys.contains(&repo_id);
                                entries.push(WorkspaceListEntry::ProjectHeader {
                                    name: label,
                                    collapse_key: repo_id.clone(),
                                    indented: true,
                                });
                                if !wt_collapsed {
                                    if let Some(members) = members_by_key.get(&repo_id) {
                                        emit_branch_subgroups(app, members, true, &mut entries);
                                    }
                                }
                            } else {
                                if let Some(space) = member_ws.git_space() {
                                    entries.push(WorkspaceListEntry::ProjectHeader {
                                        name: space.label.clone(),
                                        collapse_key: space.repo_identity.clone(),
                                        indented: true,
                                    });
                                }
                                emit_branch_subgroups(app, &[member_idx], true, &mut entries);
                            }
                        }
                    }
                }
            }
            continue;
        }

        // --- Flat (ungrouped) workspace: project header (if git) + branch bracket ---
        if let Some(space) = ws.git_space() {
            entries.push(WorkspaceListEntry::ProjectHeader {
                name: space.label.clone(),
                collapse_key: space.repo_identity.clone(),
                indented: false,
            });
        }
        emit_branch_subgroups(app, &[ws_idx], false, &mut entries);
    }
    entries
}

/// Emit branch sub-groups for a list of project-group member indices.
fn emit_branch_subgroups(
    app: &AppState,
    member_indices: &[usize],
    indented: bool,
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

    // One branch sub-tree per branch; members stack under it on the spine.
    let has_branches = !branch_order.is_empty();
    for branch in &branch_order {
        let members = &by_branch[branch];
        let (ahead, behind) = members
            .iter()
            .find_map(|&i| app.workspaces[i].git_ahead_behind())
            .unwrap_or((0, 0));
        entries.push(WorkspaceListEntry::BranchHeader {
            label: branch_display_label(branch).to_string(),
            ahead,
            behind,
            indented,
        });
        for &idx in members {
            entries.push(WorkspaceListEntry::Workspace {
                ws_idx: idx,
                indented,
                rail: BranchRail::Spine,
            });
        }
    }

    for &idx in &no_branch {
        entries.push(WorkspaceListEntry::Workspace {
            ws_idx: idx,
            indented,
            rail: BranchRail::None,
        });
    }

    // Close the project's branch sub-tree with a footer line.
    if has_branches {
        entries.push(WorkspaceListEntry::ProjectFooter { indented });
    }
}

pub(crate) fn workspace_list_rect(area: Rect, split_ratio: f32) -> Rect {
    let (ws_area, _) = expanded_sidebar_sections(area, split_ratio);
    ws_area
}

pub(crate) fn workspace_list_body_rect(area: Rect, has_scrollbar: bool) -> Rect {
    if area.width == 0 || area.height <= WORKSPACE_SECTION_HEADER_ROWS {
        return Rect::default();
    }

    let body_y = area.y.saturating_add(WORKSPACE_SECTION_HEADER_ROWS);
    let footer_y = area.y + area.height.saturating_sub(1);
    let body_height = footer_y.saturating_sub(body_y);
    let body_width = area.width.saturating_sub(u16::from(has_scrollbar));
    Rect::new(area.x, body_y, body_width, body_height)
}

fn workspace_list_visible_count(app: &AppState, area: Rect, scroll: usize) -> usize {
    let body = workspace_list_body_rect(area, false);
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
    let body = workspace_list_body_rect(area, true);
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

fn agent_panel_visible_count(area: Rect) -> usize {
    let body = agent_panel_body_rect(area, false);
    if body.width == 0 || body.height < 2 {
        return 0;
    }

    let mut used_rows = 0u16;
    let mut visible = 0usize;
    while used_rows.saturating_add(2) <= body.height {
        used_rows = used_rows.saturating_add(2);
        visible += 1;
        if used_rows < body.height {
            used_rows = used_rows.saturating_add(1);
        }
    }
    visible
}

pub(crate) fn agent_panel_scroll_metrics(app: &AppState, area: Rect) -> crate::pane::ScrollMetrics {
    let viewport_rows = agent_panel_visible_count(area);
    let total_rows = agent_panel_entries(app).len();
    let max_offset_from_bottom = total_rows.saturating_sub(viewport_rows);
    let offset_from_bottom = total_rows
        .saturating_sub(app.agent_panel_scroll)
        .saturating_sub(viewport_rows);

    crate::pane::ScrollMetrics {
        offset_from_bottom,
        max_offset_from_bottom,
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
    let body = workspace_list_body_rect(ws_area, should_show_scrollbar(metrics));
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
            WorkspaceListEntry::BranchHeader { .. } => {
                // BranchHeader is a non-clickable label — no card or header area.
            }
            WorkspaceListEntry::ProjectFooter { .. } => {
                // ProjectFooter is a non-clickable closer line — no card.
            }
            WorkspaceListEntry::Workspace {
                ws_idx, indented, ..
            } => {
                // Workspace card spans 2 rows (name + dots).
                cards.push(crate::app::state::WorkspaceCardArea {
                    ws_idx: *ws_idx,
                    rect: Rect::new(body.x, row_y, body.width, 2),
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
    let is_navigating = matches!(app.mode, Mode::Navigate);

    let p = &app.palette;
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
        let (icon, icon_style) = state_dot(agg_state, agg_seen, app.spinner_tick, p, idle_age);
        let is_selected = visible_idx == app.selected && is_navigating;
        let is_active = Some(visible_idx) == app.active;
        let row_style = if is_selected {
            Style::default().bg(p.surface0)
        } else if is_active {
            Style::default().bg(p.surface_dim)
        } else {
            Style::default()
        };
        let num_style = if is_selected {
            Style::default().fg(p.overlay1).bg(p.surface0)
        } else if is_active {
            Style::default().fg(p.text).bg(p.surface_dim)
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
                Span::styled(format!("{}", visible_idx + 1), num_style),
                Span::styled(" ", row_style),
                Span::styled(icon, icon_style),
            ])),
            Rect::new(ws_area.x, y, ws_area.width, 1),
        );
    }

    if let Some(divider_y) = divider_y {
        let buf = frame.buffer_mut();
        for x in ws_area.x..ws_area.x + ws_area.width {
            buf[(x, divider_y)].set_symbol("─");
            buf[(x, divider_y)].set_style(Style::default().fg(p.surface_dim));
        }
    }

    let detail_ws_idx = if is_navigating {
        Some(app.selected)
    } else {
        app.active
    };
    let detail_content_area = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    if detail_content_area != Rect::default() {
        if let Some(ws_idx) = detail_ws_idx {
            if let Some(ws) = app.workspaces.get(ws_idx) {
                for (detail_idx, detail) in ws.pane_details(&app.terminals).iter().enumerate() {
                    let y = detail_content_area.y + detail_idx as u16;
                    if y >= detail_content_area.y + detail_content_area.height {
                        break;
                    }
                    let pane_num = ws
                        .public_pane_number(detail.pane_id)
                        .unwrap_or(detail_idx + 1);
                    let pane_style = Style::default().fg(p.overlay0);
                    let idle_age = detail
                        .idle_since
                        .map(|since| Instant::now().saturating_duration_since(since));
                    let (icon, icon_style) =
                        agent_icon(detail.state, detail.seen, app.spinner_tick, p, idle_age);
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(format!("{pane_num}"), pane_style),
                            Span::styled(" ", pane_style),
                            Span::styled(icon, icon_style),
                        ])),
                        Rect::new(detail_content_area.x, y, detail_content_area.width, 1),
                    );
                }
            }
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
        let version_tag = concat!("v", env!("CARGO_PKG_VERSION"));
        let header_line = Line::from(vec![
            Span::styled(
                " spaces",
                Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {version_tag}"), Style::default().fg(p.overlay0)),
        ]);
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
    let body = workspace_list_body_rect(area, scrollbar_rect.is_some());
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
                        let (dot, dot_style) = state_dot(state, seen, app.spinner_tick, p, age);
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(dot, dot_style));
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::ProjectHeader {
                name,
                collapse_key,
                indented,
            } => {
                if row_y < list_bottom {
                    let collapsed = app.collapsed_space_keys.contains(collapse_key);
                    let indent = if *indented { " " } else { "" };
                    let mut spans = vec![Span::styled(
                        format!("{indent}{name}"),
                        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                    )];
                    if collapsed {
                        let (state, seen) = space_aggregate_display_state(app, collapse_key);
                        let age = space_aggregate_idle_age(app, collapse_key, now);
                        let (dot, dot_style) = state_dot(state, seen, app.spinner_tick, p, age);
                        spans.push(Span::styled(" ", Style::default()));
                        spans.push(Span::styled(dot, dot_style));
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(spans)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::BranchHeader {
                label,
                ahead,
                behind,
                indented,
            } => {
                if row_y < list_bottom {
                    let indent = if *indented { " " } else { "" };
                    let connector = "├── ";
                    let mut spans = vec![
                        Span::styled(
                            format!("{indent}{connector}"),
                            Style::default().fg(p.overlay0),
                        ),
                        Span::styled(label.clone(), Style::default().fg(p.overlay1)),
                    ];
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
                    if let Some(WorkspaceListEntry::Workspace { ws_idx, .. }) =
                        entries.get(entry_idx + 1)
                    {
                        if let Some(pr) = app
                            .workspaces
                            .get(*ws_idx)
                            .and_then(|w| w.cached_check_status.as_ref())
                            .and_then(|cs| cs.pr.as_ref())
                        {
                            let pr_color = match pr.state.as_str() {
                                "MERGED" => p.teal,
                                "CLOSED" => p.red,
                                _ => p.green,
                            };
                            spans.push(Span::styled(" ", Style::default()));
                            spans.push(Span::styled(
                                format!("#{}", pr.number),
                                Style::default().fg(pr_color),
                            ));
                        }
                    }
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

                // Card rect spans 2 rows (name + dots).
                let card_height = 2u16;
                if highlighted {
                    let bg = if selected {
                        p.surface0
                    } else if is_dragged {
                        p.surface1
                    } else {
                        p.surface_dim
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
                    Style::default().fg(p.text)
                };
                let rail_style = Style::default().fg(p.overlay0);

                // --- Line 1: name ---
                let mut line1 = Vec::new();
                let indent_prefix = if *indented { " " } else { "" };
                match rail {
                    BranchRail::Spine => {
                        line1.push(Span::styled(indent_prefix, Style::default()));
                        line1.push(Span::styled("│ ", rail_style));
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
                                let (si, ss) = state_dot(state, seen, app.spinner_tick, p, age);
                                line1.push(Span::styled(" ", Style::default()));
                                line1.push(Span::styled(si, ss));
                            }
                            line1.push(Span::styled(" ", Style::default()));
                        } else {
                            line1.push(Span::styled(indent_prefix, Style::default()));
                        }
                    }
                }

                let label = ws.display_name_from(&app.terminals, terminal_runtimes);
                line1.push(Span::styled(label, name_style));

                if row_y < list_bottom {
                    frame.render_widget(
                        Paragraph::new(Line::from(line1)),
                        Rect::new(body.x, row_y, body.width, 1),
                    );
                }

                // --- Line 2: tab dots ---
                let dots_y = row_y + 1;
                if dots_y < list_bottom {
                    let mut line2 = Vec::new();
                    match rail {
                        BranchRail::Spine => {
                            line2.push(Span::styled(indent_prefix, Style::default()));
                            line2.push(Span::styled("│ ", rail_style));
                        }
                        BranchRail::None => {
                            line2.push(Span::styled(indent_prefix, Style::default()));
                            // Align with name: extra space for non-rail.
                            if !*indented && workspace_parent_group_state(app, i).is_some() {
                                line2.push(Span::styled("  ", Style::default()));
                            }
                        }
                    }
                    let dots = tab_dot_states(ws, &app.terminals);
                    let dot_ages = tab_dot_idle_ages(ws, &app.terminals, now);
                    for (tab_idx, &(state, seen)) in dots.iter().enumerate() {
                        let (dot_glyph, mut dot_style) = state_dot(
                            state,
                            seen,
                            app.spinner_tick,
                            p,
                            dot_ages.get(tab_idx).copied().flatten(),
                        );
                        if tab_idx == ws.active_tab {
                            dot_style = dot_style.add_modifier(Modifier::BOLD);
                        }
                        if tab_idx > 0 {
                            line2.push(Span::styled(" ", Style::default()));
                        }
                        line2.push(Span::styled(dot_glyph, dot_style));
                    }
                    frame.render_widget(
                        Paragraph::new(Line::from(line2)),
                        Rect::new(body.x, dots_y, body.width, 1),
                    );
                }
            }
            WorkspaceListEntry::ProjectFooter { indented } => {
                if row_y < list_bottom {
                    let indent = if *indented { " " } else { "" };
                    frame.render_widget(
                        Paragraph::new(Line::from(Span::styled(
                            format!("{indent}╰───────"),
                            Style::default().fg(p.overlay0),
                        ))),
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
    let toggle_rect = agent_panel_toggle_rect(area, app.agent_panel_sort);
    if toggle_rect != Rect::default() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                agent_panel_sort_label(app.agent_panel_sort),
                Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
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

    let mut row_y = body.y;
    let body_bottom = body.y + body.height;
    for detail in details.iter().skip(app.agent_panel_scroll) {
        if row_y.saturating_add(1) >= body_bottom {
            break;
        }

        // Check if this agent entry corresponds to the active session
        let is_active = app.is_active_pane(detail.ws_idx, detail.tab_idx, detail.pane_id);

        let idle_age = detail
            .idle_since
            .map(|since| Instant::now().saturating_duration_since(since));
        let (icon, icon_style) =
            agent_icon(detail.state, detail.seen, app.spinner_tick, p, idle_age);
        let label_color = state_label_color(detail.state, detail.seen, p);
        let label = detail
            .state_labels
            .get(agent_panel_status_key(detail.state, detail.seen))
            .map(String::as_str)
            .unwrap_or_else(|| state_label(detail.state, detail.seen));

        let row_style = if is_active {
            Style::default().bg(p.surface_dim)
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

        let primary_label =
            format_agent_panel_primary_label(detail, body.width.saturating_sub(3) as usize);
        let name_line = Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled(icon, icon_style),
            Span::styled(" ", Style::default()),
            Span::styled(primary_label, name_style),
        ]);
        frame.render_widget(
            Paragraph::new(name_line).style(row_style),
            Rect::new(body.x, row_y, body.width, 1),
        );
        row_y += 1;

        let mut status_spans = vec![
            Span::styled("   ", Style::default()),
            Span::styled(label, status_style),
        ];
        if let Some(agent_label) = &detail.agent_label {
            status_spans.push(Span::styled(" · ", agent_style));
            status_spans.push(Span::styled(agent_label, agent_style));
        }
        if let Some(custom_status) = &detail.custom_status {
            status_spans.push(Span::styled(" · ", agent_style));
            status_spans.push(Span::styled(custom_status.clone(), agent_style));
        }
        frame.render_widget(
            Paragraph::new(Line::from(status_spans)).style(row_style),
            Rect::new(body.x, row_y, body.width, 1),
        );
        row_y += 1;

        if row_y < body_bottom {
            row_y += 1;
        }
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
    use crate::{detect::Agent, workspace::Workspace};
    use ratatui::{backend::TestBackend, Terminal};

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
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            &crate::pane::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
    fn all_workspaces_primary_label_truncates_workspace_and_tab() {
        let entry = AgentPanelEntry {
            ws_idx: 0,
            tab_idx: 0,
            pane_id: crate::layout::PaneId::from_raw(1),
            primary_label: "agent-browser".into(),
            primary_tab_label: Some("test-escalation".into()),
            agent_label: Some("claude".into()),
            state: AgentState::Idle,
            seen: true,
            idle_since: None,
            last_agent_state_change_seq: None,
            custom_status: None,
            state_labels: std::collections::HashMap::new(),
        };

        let label = format_agent_panel_primary_label(&entry, 18);

        assert_eq!(label, "agent-bro… · test…");
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
        assert!(
            text.contains("╰─"),
            "branch tree connector present: {text:?}"
        );
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
                label: "herdr".into(),
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
            label: "herdr".into(),
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
            label: "herdr".into(),
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
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/herdr"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/herdr-issue"),
            Workspace::test_new("notes"),
        ];
        app.collapsed_space_keys.insert("repo-key".into());
        app.active = None;
        app.mode = Mode::Terminal;

        let ws_area = Rect::new(0, 0, 30, 6);
        let metrics = workspace_list_scroll_metrics(&app, ws_area);

        assert_eq!(metrics.viewport_rows, 2);
        assert_eq!(metrics.max_offset_from_bottom, 2);
        assert_eq!(metrics.offset_from_bottom, 2);
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
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "other-key".into(),
                    indented: false,
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
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::None,
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
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "github.com/owner/b".into(),
                    indented: false,
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
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::None,
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
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 3,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "other-key".into(),
                    indented: false,
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
                },
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

        // Both checkouts are on branch "main" under a synthesized project header,
        // so they form one branch sub-tree: BranchHeader + two Spine members + footer.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: "github.com/owner/resume-builder".into(),
                    indented: false,
                },
                WorkspaceListEntry::BranchHeader {
                    label: "main".into(),
                    ahead: 0,
                    behind: 0,
                    indented: true,
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
                WorkspaceListEntry::ProjectFooter { indented: true },
            ]
        );
    }

    #[test]
    fn single_ws_branch_emits_bracket() {
        // A single git workspace with a branch always emits a full bracket
        // (ProjectHeader + BranchHeader + Workspace{Last}). The old "trivial"
        // short-circuit was removed.
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

        // ProjectHeader + BranchHeader for the branched child + Workspace{Last}
        // for the branched child + Workspace{None} for the no-branch parent.
        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::ProjectHeader {
                    name: "herdr".into(),
                    collapse_key: identity.into(),
                    indented: false,
                },
                WorkspaceListEntry::BranchHeader {
                    label: "main".into(),
                    ahead: 0,
                    behind: 0,
                    indented: true,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                    rail: BranchRail::Spine,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: true,
                    rail: BranchRail::None,
                },
                WorkspaceListEntry::ProjectFooter { indented: true },
            ]
        );
    }

    #[test]
    fn workspace_card_area_rect_spans_both_lines() {
        let mut app = AppState::test_new();
        app.workspaces = vec![Workspace::test_new("alpha")];

        let (cards, _) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 20));

        assert_eq!(cards.len(), 1);
        assert_eq!(
            cards[0].rect.height, 2,
            "card rect must span both name + dots lines"
        );
    }

    #[test]
    fn multiple_branches_in_one_project_emit_multiple_brackets() {
        // Each distinct branch in a project gets its own bracket.
        let mut app = AppState::test_new();
        let identity = "github.com/owner/proj";
        let mut parent = git_space_member("proj", "key-p", false);
        parent.cached_git_branch = None;
        parent.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_a = git_space_member_on_branch("feature-a", "key-a", false, "feat/a");
        ws_a.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        let mut ws_b = git_space_member_on_branch("feature-b", "key-b", false, "feat/b");
        ws_b.cached_git_space.as_mut().unwrap().repo_identity = identity.into();
        app.workspaces = vec![parent, ws_a, ws_b];

        let entries = workspace_list_entries(&app);

        // Exactly one BranchHeader, labeled by the first non-linked branched member.
        let branch_headers: Vec<_> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::BranchHeader { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            branch_headers,
            vec!["feat/a", "feat/b"],
            "one bracket per branch"
        );

        // All branch members ride the project spine down to the closer line.
        let spine_count = entries
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    WorkspaceListEntry::Workspace {
                        rail: BranchRail::Spine,
                        ..
                    }
                )
            })
            .count();
        let footer_count = entries
            .iter()
            .filter(|e| matches!(e, WorkspaceListEntry::ProjectFooter { .. }))
            .count();
        assert_eq!(spine_count, 2, "each branch member is on the spine");
        assert_eq!(footer_count, 1, "one closer line per project");
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
        }];
        assert_eq!(entry_row_height(&entries[0], &entries, 0), 1);
    }

    #[test]
    fn entry_row_height_workspace_is_two_rows() {
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
        // Every workspace is name + dots = 2 rows; no closer line.
        assert_eq!(entry_row_height(&entries[0], &entries, 0), 2);
        assert_eq!(entry_row_height(&entries[1], &entries, 1), 2);
    }
}
