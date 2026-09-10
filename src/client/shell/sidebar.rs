use super::*;
use ratatui::{
    text::Line,
    widgets::{Paragraph, Widget},
};

pub(in crate::client::shell) fn collapsed_sidebar_sections(
    area: Rect,
) -> (Rect, Option<u16>, Rect) {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        return (Rect::default(), None, Rect::default());
    }
    if content.height < 7 {
        return (content, None, Rect::default());
    }
    let workspace_height = content.height.div_ceil(2);
    let divider_y = content.y + workspace_height;
    let detail_height = content.height.saturating_sub(workspace_height + 1);
    (
        Rect::new(content.x, content.y, content.width, workspace_height),
        Some(divider_y),
        Rect::new(content.x, divider_y + 1, content.width, detail_height),
    )
}

pub(crate) fn render_collapsed_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    selected_workspace_id: Option<&str>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    render_sidebar_background(buffer, area, palette);
    let (workspace_area, divider_y, detail_area) = collapsed_sidebar_sections(area);
    for (index, workspace) in snapshot
        .workspaces
        .iter()
        .take(workspace_area.height as usize)
        .enumerate()
    {
        let rect = Rect::new(
            workspace_area.x,
            workspace_area.y + index as u16,
            workspace_area.width,
            1,
        );
        let selected = selected_workspace_id == Some(workspace.workspace_id.as_str());
        let selection_background =
            if workspace.focused && palette.selection_bg == ratatui::style::Color::Reset {
                palette.active_row_bg
            } else {
                palette.selection_bg
            };
        if selected {
            buffer.set_style(rect, Style::default().bg(selection_background));
        } else if workspace.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        let number_style = if selected {
            Style::default()
                .fg(palette.overlay1)
                .bg(selection_background)
        } else if workspace.focused {
            Style::default().fg(palette.text).bg(palette.active_row_bg)
        } else {
            Style::default().fg(palette.overlay0)
        };
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &format!("{:<2}", index + 1),
            number_style,
        );
        let status = workspace.agent_status;
        put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            status_icon(status, config.status_indicators),
            Style::default().fg(status_color(status, palette)),
        );
        hits.workspaces.push(WorkspaceHit {
            rect,
            endpoint_id: ClientEndpointId::Local,
            workspace_id: workspace.workspace_id.clone(),
            indented: false,
            group_toggle: None,
        });
    }

    if let Some(divider_y) = divider_y {
        put_text(
            buffer,
            workspace_area.x,
            divider_y,
            workspace_area.width,
            &"─".repeat(workspace_area.width as usize),
            Style::default().fg(palette.surface_dim),
        );
    }

    let detail_content = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    for (index, pane_id) in super::ordered_agent_pane_ids(snapshot, config.agent_panel_sort)
        .into_iter()
        .take(detail_content.height as usize)
        .enumerate()
    {
        let Some(agent) = snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
        else {
            continue;
        };
        let rect = Rect::new(
            detail_content.x,
            detail_content.y + index as u16,
            detail_content.width,
            1,
        );
        if agent.focused {
            buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
        }
        put_text(
            buffer,
            rect.x,
            rect.y,
            rect.width.min(2),
            &format!("{:<2}", index + 1),
            Style::default().fg(if agent.focused {
                palette.text
            } else {
                palette.overlay0
            }),
        );
        put_text(
            buffer,
            rect.x.saturating_add(2),
            rect.y,
            rect.width.saturating_sub(2),
            status_icon(agent.agent_status, config.status_indicators),
            Style::default().fg(status_color(agent.agent_status, palette)),
        );
        hits.agents.push((rect, pane_id));
    }
    hits.sidebar_toggle = if area.is_empty() || workspace_area.width == 0 {
        Rect::default()
    } else {
        Rect::new(
            workspace_area.x + workspace_area.width / 2,
            area.bottom().saturating_sub(1),
            1,
            1,
        )
    };
    put_text(
        buffer,
        hits.sidebar_toggle.x,
        hits.sidebar_toggle.y,
        hits.sidebar_toggle.width,
        "»",
        if super::super::global_menu::global_menu_attention(snapshot) {
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette.overlay0)
        },
    );
}

pub(crate) fn render_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    render_sidebar_background(buffer, area, palette);
    hits.sidebar_divider = if area.is_empty() {
        Rect::default()
    } else {
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height)
    };
    let (workspace_area, detail_area) =
        crate::ui::expanded_sidebar_sections(area, state.sidebar_section_split);
    hits.sidebar_section_divider =
        crate::ui::sidebar_section_divider_rect(area, state.sidebar_section_split);
    put_text(
        buffer,
        workspace_area.x,
        workspace_area.y,
        workspace_area.width,
        " spaces",
        Style::default()
            .fg(palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    // Aggregate attention badge (ceo-bora#302): right-aligned on the same
    // header row as " spaces" so `WORKSPACE_HEADER_ROWS` -- and therefore
    // `body`/`hits.workspace_body` below -- keeps its layout untouched.
    let (waiting, blocked) = attention::attention_counts(snapshot, config.idle_attention_seconds);
    if waiting > 0 {
        let badge = format!("{waiting} waiting");
        let badge_width = display_width(&badge);
        // " spaces" is 7 columns; keep at least one blank column between it
        // and the badge, otherwise the header is too narrow to say both.
        if workspace_area.width >= badge_width.saturating_add(8) {
            put_text(
                buffer,
                workspace_area.right().saturating_sub(badge_width),
                workspace_area.y,
                badge_width,
                &badge,
                Style::default()
                    .fg(if blocked > 0 {
                        palette.red
                    } else {
                        palette.yellow
                    })
                    .add_modifier(Modifier::BOLD),
            );
        }
    }

    let body = Rect::new(
        workspace_area.x,
        workspace_area.y.saturating_add(WORKSPACE_HEADER_ROWS),
        workspace_area.width,
        workspace_area
            .height
            .saturating_sub(WORKSPACE_HEADER_ROWS + 1),
    );
    hits.workspace_body = body;
    if state.view_mode == crate::config::ViewMode::Folders {
        render_folders_workspace_list(buffer, body, snapshot, config, state, hits);
    } else {
        let entries = match state.view_mode {
            // Flat: one row per workspace in workspace-vec order, no
            // grouping at all -- repo brackets and worktree auto-grouping
            // both dissolve while this is selected.
            crate::config::ViewMode::Flat => (0..snapshot.workspaces.len())
                .map(|index| WorkspaceEntry {
                    index,
                    indented: false,
                    last_child: false,
                })
                .collect::<Vec<_>>(),
            // Repo (default): unchanged upstream behavior, dispatched here
            // rather than mutated in place.
            crate::config::ViewMode::Repo | crate::config::ViewMode::Folders => {
                workspace_entries(snapshot, state.collapsed_groups)
            }
        };
        let row_heights = entries
            .iter()
            .map(|entry| {
                snapshot
                    .workspaces
                    .get(entry.index)
                    .map(|workspace| {
                        workspace_rows(
                            workspace,
                            displayed_workspace_status(snapshot, workspace, state.collapsed_groups),
                            entry.indented,
                            &config.spaces,
                        )
                        .len()
                        .max(1)
                        .min(u16::MAX as usize) as u16
                    })
                    .unwrap_or(1)
            })
            .collect::<Vec<_>>();
        let gaps = entries
            .iter()
            .enumerate()
            .map(|(index, _)| {
                entries
                    .get(index + 1)
                    .map_or(0, |next| u16::from(!next.indented) * config.spaces.row_gap)
            })
            .collect::<Vec<_>>();
        let mut metrics = super::scroll::list_scroll_metrics(
            &row_heights,
            &gaps,
            body.height,
            *state.workspace_scroll,
        );
        if !body.is_empty() && std::mem::take(state.reveal_focused_workspace) {
            if let Some(target) = entries
                .iter()
                .position(|entry| snapshot.workspaces[entry.index].focused)
            {
                *state.workspace_scroll = super::scroll::list_scroll_start_to_reveal(
                    &row_heights,
                    &gaps,
                    body.height,
                    *state.workspace_scroll,
                    target,
                );
                metrics = super::scroll::list_scroll_metrics(
                    &row_heights,
                    &gaps,
                    body.height,
                    *state.workspace_scroll,
                );
            }
        }
        hits.workspace_max_scroll = metrics.max_offset_from_bottom;
        hits.workspace_scroll_metrics = Some(metrics);
        *state.workspace_scroll = metrics
            .max_offset_from_bottom
            .saturating_sub(metrics.offset_from_bottom);
        let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
        let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
        let mut y = body.y;
        for (entry_position, entry) in entries.iter().enumerate().skip(*state.workspace_scroll) {
            let Some(workspace) = snapshot.workspaces.get(entry.index) else {
                continue;
            };
            let status = displayed_workspace_status(snapshot, workspace, state.collapsed_groups);
            let rows = workspace_rows(workspace, status, entry.indented, &config.spaces);
            let row_height = (rows.len().max(1).min(u16::MAX as usize) as u16).min(body.height);
            if y.saturating_add(row_height) > body.bottom() {
                break;
            }
            let rect = Rect::new(body.x, y, content_width, row_height);
            let selected = state.selected_workspace_id == Some(workspace.workspace_id.as_str());
            let dragged = state.dragged_workspace_id == Some(workspace.workspace_id.as_str());
            if selected {
                buffer.set_style(rect, Style::default().bg(palette.selection_bg));
            } else if dragged {
                buffer.set_style(rect, Style::default().bg(palette.surface1));
            } else if workspace.focused {
                buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
            }
            render_workspace_rows(
                buffer,
                rect,
                workspace,
                status,
                config.status_indicators,
                entry,
                rows,
                WorkspaceRowRenderOptions {
                    endpoint_active: true,
                    selected,
                    dragged,
                    first_row_reserved_width: 0,
                    tick: state.tick,
                },
                palette,
            );
            let group_toggle = render_parent_group_toggle(
                buffer,
                rect,
                snapshot,
                entry.index,
                state.collapsed_groups,
                palette,
            );
            hits.workspaces.push(WorkspaceHit {
                rect,
                endpoint_id: ClientEndpointId::Local,
                workspace_id: workspace.workspace_id.clone(),
                indented: entry.indented,
                group_toggle,
            });
            let gap = entries
                .get(entry_position + 1)
                .map_or(0, |next| u16::from(!next.indented) * config.spaces.row_gap);
            y = y.saturating_add(row_height + gap);
        }

        if show_scrollbar {
            let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
            hits.workspace_scrollbar = track;
            super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
        }
    }

    if let Some(row) = state.workspace_drop_indicator_row.filter(|row| {
        *row >= workspace_area.y.saturating_add(1)
            && *row < workspace_area.bottom().saturating_sub(1)
    }) {
        put_text(
            buffer,
            body.x,
            row,
            body.width,
            &"─".repeat(body.width as usize),
            Style::default().fg(palette.accent),
        );
    }

    let footer_y = workspace_area.bottom().saturating_sub(1);
    if config.mouse_capture {
        hits.new_workspace = Rect::new(
            workspace_area.x,
            footer_y,
            5.min(workspace_area.width),
            u16::from(workspace_area.height > 0),
        );
        put_text(
            buffer,
            workspace_area.x,
            footer_y,
            workspace_area.width,
            " new",
            Style::default().fg(palette.overlay0),
        );
        let attention = super::super::global_menu::global_menu_attention(snapshot);
        let launcher_width = if attention { 8 } else { 6 }.min(workspace_area.width);
        hits.global_launcher = Rect::new(
            workspace_area.right().saturating_sub(launcher_width),
            footer_y,
            launcher_width,
            1,
        );
        if attention {
            let start_x = workspace_area.right().saturating_sub(6);
            put_text(
                buffer,
                start_x,
                footer_y,
                2,
                "● ",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            );
            put_text(
                buffer,
                start_x.saturating_add(2),
                footer_y,
                4,
                "menu",
                Style::default().fg(palette.overlay0),
            );
        } else {
            put_right_text(
                buffer,
                workspace_area,
                footer_y,
                "menu",
                Style::default().fg(palette.overlay0),
            );
        }
    }

    super::render_agent_panel(
        buffer,
        detail_area,
        snapshot,
        config,
        state.agent_scroll,
        hits,
    );

    hits.sidebar_toggle = Rect::new(
        area.right().saturating_sub(2),
        area.bottom().saturating_sub(1),
        u16::from(area.width > 1),
        u16::from(area.height > 0),
    );
    put_text(
        buffer,
        hits.sidebar_toggle.x,
        hits.sidebar_toggle.y,
        hits.sidebar_toggle.width,
        "«",
        Style::default().fg(palette.overlay0),
    );
}

pub(crate) fn workspace_entries(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
) -> Vec<WorkspaceEntry> {
    let mut members = HashMap::<&str, Vec<usize>>::new();
    for (index, workspace) in snapshot.workspaces.iter().enumerate() {
        if let Some(worktree) = &workspace.worktree {
            members.entry(&worktree.key).or_default().push(index);
        }
    }
    let grouped = members
        .iter()
        .filter(|(_, indices)| {
            indices.len() >= 2
                && indices.iter().any(|index| {
                    snapshot.workspaces[*index]
                        .worktree
                        .as_ref()
                        .is_some_and(|worktree| !worktree.is_linked_worktree)
                })
        })
        .map(|(key, _)| *key)
        .collect::<HashSet<_>>();
    let mut emitted = HashSet::<&str>::new();
    let mut entries = Vec::new();
    for (index, workspace) in snapshot.workspaces.iter().enumerate() {
        let Some(worktree) = workspace
            .worktree
            .as_ref()
            .filter(|worktree| grouped.contains(worktree.key.as_str()))
        else {
            entries.push(WorkspaceEntry {
                index,
                indented: false,
                last_child: false,
            });
            continue;
        };
        if !emitted.insert(&worktree.key) {
            continue;
        }
        let Some(group_members) = members.get(worktree.key.as_str()) else {
            continue;
        };
        let parent = group_members
            .iter()
            .copied()
            .find(|member| {
                snapshot.workspaces[*member]
                    .worktree
                    .as_ref()
                    .is_some_and(|worktree| !worktree.is_linked_worktree)
            })
            .unwrap_or(index);
        entries.push(WorkspaceEntry {
            index: parent,
            indented: false,
            last_child: false,
        });
        if collapsed_groups.contains(&worktree.key) {
            if let Some(active) = group_members
                .iter()
                .copied()
                .find(|member| *member != parent && snapshot.workspaces[*member].focused)
            {
                entries.push(WorkspaceEntry {
                    index: active,
                    indented: true,
                    last_child: true,
                });
            }
            continue;
        }
        let children = group_members
            .iter()
            .copied()
            .filter(|member| *member != parent)
            .collect::<Vec<_>>();
        for (child_index, child) in children.iter().enumerate() {
            entries.push(WorkspaceEntry {
                index: *child,
                indented: true,
                last_child: child_index + 1 == children.len(),
            });
        }
    }
    entries
}

fn parent_group_key(snapshot: &ClientShellSnapshot, index: usize) -> Option<String> {
    let workspace = snapshot.workspaces.get(index)?;
    let worktree = workspace.worktree.as_ref()?;
    if worktree.is_linked_worktree {
        return None;
    }
    (snapshot
        .workspaces
        .iter()
        .filter(|candidate| {
            candidate
                .worktree
                .as_ref()
                .is_some_and(|candidate| candidate.key == worktree.key)
        })
        .count()
        >= 2)
        .then(|| worktree.key.clone())
}

pub(in crate::client::shell) fn render_parent_group_toggle(
    buffer: &mut Buffer,
    workspace_rect: Rect,
    snapshot: &ClientShellSnapshot,
    workspace_index: usize,
    collapsed_groups: &HashSet<String>,
    palette: &Palette,
) -> Option<(Rect, String)> {
    let key = parent_group_key(snapshot, workspace_index)?;
    let toggle = Rect::new(
        workspace_rect.right().saturating_sub(1),
        workspace_rect.y,
        1,
        1,
    );
    put_text(
        buffer,
        toggle.x,
        toggle.y,
        toggle.width,
        if collapsed_groups.contains(&key) {
            "▸"
        } else {
            "▾"
        },
        Style::default().fg(palette.accent),
    );
    Some((toggle, key))
}

pub(in crate::client::shell) fn displayed_workspace_status(
    snapshot: &ClientShellSnapshot,
    workspace: &ClientShellWorkspace,
    collapsed_groups: &HashSet<String>,
) -> crate::api::schema::AgentStatus {
    let Some(worktree) = workspace
        .worktree
        .as_ref()
        .filter(|worktree| !worktree.is_linked_worktree)
    else {
        return workspace.agent_status;
    };
    if !collapsed_groups.contains(&worktree.key) {
        return workspace.agent_status;
    }
    snapshot
        .workspaces
        .iter()
        .filter(|candidate| {
            candidate
                .worktree
                .as_ref()
                .is_some_and(|candidate| candidate.key == worktree.key)
        })
        .map(|candidate| candidate.agent_status)
        .max_by_key(|status| status_priority(*status))
        .unwrap_or(workspace.agent_status)
}

pub(in crate::client::shell) fn workspace_rows(
    workspace: &ClientShellWorkspace,
    status: crate::api::schema::AgentStatus,
    indented: bool,
    config: &SpacesSidebarConfig,
) -> Vec<Vec<crate::ui::ResolvedToken>> {
    let label = if indented && !workspace.custom_label {
        workspace
            .branch
            .as_deref()
            .and_then(|branch| branch.strip_prefix("worktree/").or(Some(branch)))
            .unwrap_or(&workspace.label)
    } else {
        &workspace.label
    };
    let token_values = workspace.tokens.iter().cloned().collect::<HashMap<_, _>>();
    crate::ui::sidebar_space_rows(
        config,
        crate::ui::SpaceTokenContext {
            workspace: label,
            branch: workspace.branch.as_deref(),
            state_text: status_text(status),
            ahead_behind: workspace.git_ahead_behind,
            tokens: &token_values,
            suppress_git_details: indented,
        },
    )
}

/// Per-call render flags for `render_workspace_rows`, bundled to keep the
/// function under clippy's argument-count lint (ceo-bora#302 cubic review).
pub(in crate::client::shell) struct WorkspaceRowRenderOptions {
    pub(in crate::client::shell) endpoint_active: bool,
    pub(in crate::client::shell) selected: bool,
    pub(in crate::client::shell) dragged: bool,
    /// Columns reserved on ROW 0 ONLY (ceo-bora#302 cubic review): a
    /// Folders entry's pane dots share row 0 with the row-template text, so
    /// only that row needs its width narrowed -- every other row already
    /// has the full `area.width` to itself and must not be clipped for a
    /// strip that never paints there. Flat/Repo pass 0 (no dots on any row).
    pub(in crate::client::shell) first_row_reserved_width: u16,
    /// Animated-spinner frame counter (ceo-bora#303), forwarded to
    /// `status_icon_animated` for row 0's leading glyph.
    pub(in crate::client::shell) tick: u32,
}

pub(in crate::client::shell) fn render_workspace_rows(
    buffer: &mut Buffer,
    area: Rect,
    workspace: &ClientShellWorkspace,
    status: crate::api::schema::AgentStatus,
    indicators: crate::config::StatusIndicatorStyle,
    entry: &WorkspaceEntry,
    rows: Vec<Vec<crate::ui::ResolvedToken>>,
    options: WorkspaceRowRenderOptions,
    palette: &Palette,
) {
    let WorkspaceRowRenderOptions {
        endpoint_active,
        selected,
        dragged,
        first_row_reserved_width,
        tick,
    } = options;
    for (row_index, row) in rows.iter().enumerate() {
        let y = area.y + row_index as u16;
        if y >= area.bottom() {
            break;
        }
        let mut x = area.x;
        if entry.indented {
            let prefix = if row_index == 0 {
                if entry.last_child {
                    "   └─ "
                } else {
                    "   ├─ "
                }
            } else if entry.last_child {
                "        "
            } else {
                "   │    "
            };
            x = put_segment(
                buffer,
                x,
                y,
                area.right(),
                prefix,
                Style::default().fg(palette.overlay0),
            );
        } else if row_index == 0 {
            x = x.saturating_add(1);
        } else {
            x = x.saturating_add(3);
        }
        let row_reserved = if row_index == 0 {
            first_row_reserved_width
        } else {
            0
        };
        let highlighted = endpoint_active && workspace.focused || dragged;
        let workspace_style = Style::default()
            .fg(if highlighted {
                palette.text
            } else {
                palette.subtext0
            })
            .add_modifier(if highlighted {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        let secondary_style = Style::default().fg(if endpoint_active && workspace.focused {
            palette.mauve
        } else {
            palette.overlay0
        });
        let row_width = area
            .right()
            .saturating_sub(2)
            .saturating_sub(x)
            .saturating_sub(row_reserved);
        let spans = crate::ui::resolved_token_spans(
            row,
            (
                status_icon_animated(status, indicators, tick),
                Style::default().fg(status_color(status, palette)),
            ),
            Style::default()
                .fg(status_color(status, palette))
                .add_modifier(Modifier::DIM),
            workspace_style,
            secondary_style,
            Style::default().fg(palette.overlay1),
            palette,
            row_width as usize,
        );
        Paragraph::new(Line::from(spans)).render(Rect::new(x, y, row_width, 1), buffer);
    }

    let background = if selected {
        Some(palette.selection_bg)
    } else if dragged {
        Some(palette.surface1)
    } else if endpoint_active && workspace.focused {
        Some(palette.active_row_bg)
    } else {
        None
    };
    if let Some(background) = background {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                buffer[(x, y)].set_bg(background);
            }
        }
    }
}

/// Folders view (`ViewMode::Folders`, ceo-bora#275, reshaped by
/// ceo-bora#303): a workspace list that honors only user-defined
/// `visual_group` folders. No repo auto-grouping and no branch brackets,
/// unlike `ViewMode::Repo`.
///
/// `visual_group` is read as a `/`-separated PATH -- a purely client-side
/// reinterpretation of the same wire string, no protocol change:
/// `"alpha"` is a top-level folder, `"bora-sync/docs"` is a `docs` folder
/// nested one level under `bora-sync`. A parent path that no workspace
/// sits on directly still gets a header row; it is synthesized from its
/// descendants.
///
/// Owner's ruling (ceo-bora#303, 2026-09-10): "membros de pasta tem recuo
/// proprio (por nivel, para o aninhamento), e a lista se organiza como o
/// `folders_view_entries` do fork fazia -- soltos primeiro, depois cada
/// pasta com seus membros -- nunca uma linha solta abaixo de um cabecalho
/// sem separacao visual." So the order is: every ungrouped ("loose")
/// workspace first, keeping its relative `snapshot.workspaces` order,
/// then each top-level folder in order of first appearance among grouped
/// workspaces; a folder is its header, then its DIRECT members (again in
/// workspace order), then each subfolder, applying the same rule
/// recursively. A folder is NOT anchored at its first member's position
/// in workspace-vec order any more -- that older behavior is exactly what
/// left a loose row sitting under a header at the same indent, reading as
/// if it belonged to that folder.
pub(in crate::client::shell) enum FoldersRow {
    GroupHeader {
        /// FULL `/`-separated path. The header paints only
        /// `group_display_name` of it, but the whole path is what the
        /// collapse key and `WorkspaceSetGroup` need.
        name: String,
        collapse_key: String,
        /// `group_depth` of `name`: 0 for a top-level folder, 1 for a
        /// subfolder of one, and so on.
        depth: u16,
    },
    /// `depth` is carried on the entry (rather than re-derived at render
    /// time) so `folders_row_gap`, the row-height pass and the render loop
    /// agree on indentation and on which rows stay glued to the row above
    /// them, without a second lookup into the folder map. 0 is a loose
    /// workspace, 1 a direct member of a top-level folder, 2 a member one
    /// level deeper.
    Workspace { index: usize, depth: u16 },
}

/// How many `/`-separated levels sit above `path`'s own segment: 0 for a
/// top-level folder, 1 for `"bora-sync/docs"`.
fn group_depth(path: &str) -> u16 {
    path.matches('/').count().min(u16::MAX as usize) as u16
}

/// The last `/`-separated segment -- what a nested header paints, so
/// `"bora-sync/docs"` shows `docs` instead of repeating its ancestry.
fn group_display_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Everything before the last `/`, or `None` when `path` is top-level.
fn group_parent_path(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(parent, _)| parent)
}

pub(in crate::client::shell) fn folders_entries(
    snapshot: &ClientShellSnapshot,
    collapsed_groups: &HashSet<String>,
) -> Vec<FoldersRow> {
    // Loose workspaces first, relative order untouched.
    let mut entries = snapshot
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, workspace)| workspace.visual_group.is_none())
        .map(|(index, _)| FoldersRow::Workspace { index, depth: 0 })
        .collect::<Vec<_>>();
    // `direct` holds the workspaces sitting on a path EXACTLY; `group_paths`
    // holds every folder that exists at all -- ancestors synthesized by
    // walking `group_parent_path` up from each member -- ordered by the
    // first grouped workspace that mentions them, ancestors before
    // descendants.
    let mut direct: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut group_paths: Vec<&str> = Vec::new();
    for (index, workspace) in snapshot.workspaces.iter().enumerate() {
        let Some(group) = workspace.visual_group.as_deref() else {
            continue;
        };
        direct.entry(group).or_default().push(index);
        let mut ancestry = Vec::new();
        let mut cursor = Some(group);
        while let Some(path) = cursor {
            ancestry.push(path);
            cursor = group_parent_path(path);
        }
        for path in ancestry.into_iter().rev() {
            if !group_paths.contains(&path) {
                group_paths.push(path);
            }
        }
    }
    for path in group_paths
        .iter()
        .copied()
        .filter(|path| group_parent_path(path).is_none())
    {
        push_folders_group(path, &group_paths, &direct, collapsed_groups, &mut entries);
    }
    entries
}

/// One folder subtree, depth-first: header, own direct members, then each
/// subfolder (ceo-bora#303 ordering). A collapsed folder emits its header
/// and nothing below it -- members and subfolders alike -- which is how
/// collapse keeps working unchanged for nested paths.
fn push_folders_group<'a>(
    path: &'a str,
    group_paths: &[&'a str],
    direct: &HashMap<&'a str, Vec<usize>>,
    collapsed_groups: &HashSet<String>,
    entries: &mut Vec<FoldersRow>,
) {
    let depth = group_depth(path);
    let collapse_key = format!("vg:{path}");
    entries.push(FoldersRow::GroupHeader {
        name: path.to_owned(),
        collapse_key: collapse_key.clone(),
        depth,
    });
    if collapsed_groups.contains(&collapse_key) {
        return;
    }
    for &index in direct.get(path).into_iter().flatten() {
        entries.push(FoldersRow::Workspace {
            index,
            depth: depth.saturating_add(1),
        });
    }
    for child in group_paths
        .iter()
        .copied()
        .filter(|candidate| group_parent_path(candidate) == Some(path))
    {
        push_folders_group(child, group_paths, direct, collapsed_groups, entries);
    }
}

/// Columns a Folders row is pushed right per nesting level (ceo-bora#303):
/// two per level, so a folder's members read as clearly inside their
/// header and a loose row -- always at column 0 -- can never be mistaken
/// for one, which is the bug the owner reported on 2026-09-10. Clamped
/// against the row width so a very deep path cannot eat the whole row on
/// a narrow sidebar.
fn folders_depth_indent(depth: u16, width: u16) -> u16 {
    depth.saturating_mul(2).min(width / 2)
}

/// Row gap shared by the scroll-metrics pass and the render loop
/// (lockstep, same shape as `entry_row_height` in the pre-merge fork):
/// anything that lives INSIDE a folder -- a member row or a nested
/// subfolder header, i.e. any entry at `depth > 0` -- stays glued to the
/// row above it (gap 0), so a subtree paints as one block. Everything
/// else, top-level folder headers and loose rows included, gets
/// `config.spaces.row_gap`, which is what keeps a loose row from ever
/// touching a header (ceo-bora#303).
fn folders_row_gap(entries: &[FoldersRow], index: usize, row_gap: u16) -> u16 {
    match entries.get(index + 1) {
        Some(FoldersRow::Workspace { depth, .. } | FoldersRow::GroupHeader { depth, .. }) => {
            if *depth > 0 {
                0
            } else {
                row_gap
            }
        }
        None => 0,
    }
}

/// One `(status, idle_seconds)` pair per pane in `workspace_id`, ordered as
/// `snapshot.panes` lists them. The status reuses the same `agent_status`
/// mapping the collapsed sidebar and agent panel already read off
/// `snapshot.agents` -- a pane with no matching agent (a plain shell) is
/// `AgentStatus::Unknown` -- and `idle_seconds` comes straight off the pane
/// the server already ships. Thresholding those seconds into a color is a
/// paint-time decision, made by `attention::pane_attention_color`
/// (ceo-bora#302), never here.
fn workspace_pane_dot_states(
    snapshot: &ClientShellSnapshot,
    workspace_id: &str,
) -> Vec<(crate::api::schema::AgentStatus, Option<u64>)> {
    snapshot
        .panes
        .iter()
        .filter(|pane| pane.workspace_id == workspace_id)
        .map(|pane| {
            let status = snapshot
                .agents
                .iter()
                .find(|agent| agent.pane_id == pane.pane_id)
                .map(|agent| agent.agent_status)
                .unwrap_or(crate::api::schema::AgentStatus::Unknown);
            (status, pane.idle_seconds)
        })
        .collect()
}

/// Columns reserved on row 0 of a Folders workspace entry for its pane dots:
/// `dots * 2 - 1` cells (`○ ○`) plus one separating column before the text,
/// or 0 when there are no dots at all (no panes, or `ui.hide_pane_badges`).
/// Derived from the dot data itself, so the text rect and the dot strip can
/// never disagree about where the boundary sits.
fn folders_dots_reserved_width(dots: usize) -> u16 {
    if dots == 0 {
        return 0;
    }
    ((dots * 2 - 1).min(u16::MAX as usize) as u16).saturating_add(1)
}

/// Owner's ruling (2026-08-31): name and dots share ONE row, `name ○ ○`,
/// dots right-aligned -- which, now that a Folders entry renders its full
/// `[ui.sidebar.spaces].rows` template, means row 0 of that entry. The glyph
/// stays `status_icon`; only the color ramps with pane attention
/// (ceo-bora#302), so a pane silent past `ui.idle_attention_seconds` turns
/// yellow and a blocked one red.
fn render_folders_pane_dots(
    buffer: &mut Buffer,
    rect: Rect,
    dots: &[(crate::api::schema::AgentStatus, Option<u64>)],
    indicators: crate::config::StatusIndicatorStyle,
    idle_attention_seconds: u64,
    workspace_focused: bool,
    tick: u32,
    palette: &Palette,
) {
    let dots_width = folders_dots_reserved_width(dots.len()).saturating_sub(1);
    let mut dot_x = rect.right().saturating_sub(dots_width);
    for (index, (status, idle_seconds)) in dots.iter().enumerate() {
        if index > 0 {
            dot_x = dot_x.saturating_add(1);
        }
        dot_x = put_segment(
            buffer,
            dot_x,
            rect.y,
            rect.right(),
            status_icon_animated(*status, indicators, tick),
            Style::default()
                .fg(attention::pane_attention_color(
                    *status,
                    *idle_seconds,
                    idle_attention_seconds,
                    workspace_focused,
                    palette,
                ))
                .add_modifier(Modifier::BOLD),
        );
    }
}

/// Whole-body Folders render path: its own entries, scroll metrics, and
/// hit areas, independent of the Flat/Repo loop above (aceite #5: the
/// Repo path stays byte-identical to upstream, so Folders never shares its
/// row logic). Writes into the same `hits.workspace_*`/`workspace_scroll`
/// fields as the Flat/Repo path so the shared footer/agent-panel epilogue
/// in `render_sidebar` lines up regardless of which branch ran.
pub(in crate::client::shell) fn render_folders_workspace_list(
    buffer: &mut Buffer,
    body: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    let entries = folders_entries(snapshot, state.collapsed_groups);
    // Row heights computed ONCE and reused by both the scroll-metrics pass
    // below and the render loop (lockstep: a second computation is a second
    // chance to disagree). A workspace row is as tall as its resolved
    // `[ui.sidebar.spaces].rows` template -- the same template Flat/Repo
    // render, ceo-bora#302 -- and a group header is always one row. Pane
    // dots share row 0, so they cost width, never height.
    let row_heights = entries
        .iter()
        .map(|entry| match entry {
            FoldersRow::GroupHeader { .. } => 1,
            FoldersRow::Workspace { index, .. } => snapshot
                .workspaces
                .get(*index)
                .map(|workspace| {
                    workspace_rows(
                        workspace,
                        displayed_workspace_status(snapshot, workspace, state.collapsed_groups),
                        false,
                        &config.spaces,
                    )
                    .len()
                    .max(1)
                    .min(u16::MAX as usize) as u16
                })
                .unwrap_or(1),
        })
        .collect::<Vec<_>>();
    let gaps = (0..entries.len())
        .map(|index| folders_row_gap(&entries, index, config.spaces.row_gap))
        .collect::<Vec<_>>();
    let mut metrics = super::scroll::list_scroll_metrics(
        &row_heights,
        &gaps,
        body.height,
        *state.workspace_scroll,
    );
    if !body.is_empty() && std::mem::take(state.reveal_focused_workspace) {
        if let Some(target) = entries.iter().position(|entry| {
            matches!(entry, FoldersRow::Workspace { index, .. } if snapshot.workspaces[*index].focused)
        }) {
            *state.workspace_scroll = super::scroll::list_scroll_start_to_reveal(
                &row_heights,
                &gaps,
                body.height,
                *state.workspace_scroll,
                target,
            );
            metrics = super::scroll::list_scroll_metrics(
                &row_heights,
                &gaps,
                body.height,
                *state.workspace_scroll,
            );
        }
    }
    hits.workspace_max_scroll = metrics.max_offset_from_bottom;
    hits.workspace_scroll_metrics = Some(metrics);
    *state.workspace_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (position, entry) in entries.iter().enumerate().skip(*state.workspace_scroll) {
        if y >= body.bottom() {
            break;
        }
        let row_height = row_heights
            .get(position)
            .copied()
            .unwrap_or(1)
            .min(body.height);
        match entry {
            FoldersRow::GroupHeader {
                name,
                collapse_key,
                depth,
            } => {
                // The HIT rect stays the full row (click and collapse toggle
                // are unchanged at any depth); only the painted chevron and
                // label move right with the nesting level.
                let rect = Rect::new(body.x, y, content_width, 1);
                let indent = folders_depth_indent(*depth, content_width);
                let collapsed = state.collapsed_groups.contains(collapse_key);
                let chevron = if collapsed { "▸" } else { "▾" };
                let x = put_segment(
                    buffer,
                    rect.x.saturating_add(indent),
                    rect.y,
                    rect.right(),
                    chevron,
                    Style::default().fg(palette.accent),
                )
                .saturating_add(1);
                put_text(
                    buffer,
                    x,
                    rect.y,
                    rect.right().saturating_sub(x),
                    group_display_name(name),
                    Style::default()
                        .fg(palette.overlay0)
                        .add_modifier(Modifier::BOLD),
                );
                hits.folders_group_headers
                    .push((rect, collapse_key.clone()));
            }
            FoldersRow::Workspace { index, depth } => {
                let Some(workspace) = snapshot.workspaces.get(*index) else {
                    continue;
                };
                if y.saturating_add(row_height) > body.bottom() {
                    break;
                }
                let rect = Rect::new(body.x, y, content_width, row_height);
                // Nesting indent (ceo-bora#303): the row's HIT rect, its
                // highlight and its right-aligned pane dots all keep the
                // full row -- unchanged behavior -- while the row-template
                // text starts `indent` columns further right, so a member
                // of a folder can never line up with a loose row.
                let indent = folders_depth_indent(*depth, content_width);
                let text_rect = Rect::new(
                    rect.x.saturating_add(indent),
                    rect.y,
                    rect.width.saturating_sub(indent),
                    row_height,
                );
                // `hide_pane_badges` suppresses the dots outright: no dot
                // states are computed and the whole row width goes to text.
                let dots = if config.hide_pane_badges {
                    Vec::new()
                } else {
                    workspace_pane_dot_states(snapshot, &workspace.workspace_id)
                };
                let reserved = folders_dots_reserved_width(dots.len());
                let selected = state.selected_workspace_id == Some(workspace.workspace_id.as_str());
                let dragged = state.dragged_workspace_id == Some(workspace.workspace_id.as_str());
                // Highlight spans every row of the entry, dot strip
                // included -- same shape as the Flat/Repo loop above.
                if selected {
                    buffer.set_style(rect, Style::default().bg(palette.selection_bg));
                } else if dragged {
                    buffer.set_style(rect, Style::default().bg(palette.surface1));
                } else if workspace.focused {
                    buffer.set_style(rect, Style::default().bg(palette.active_row_bg));
                }
                let status =
                    displayed_workspace_status(snapshot, workspace, state.collapsed_groups);
                let rows = workspace_rows(workspace, status, false, &config.spaces);
                render_workspace_rows(
                    buffer,
                    text_rect,
                    workspace,
                    status,
                    config.status_indicators,
                    &WorkspaceEntry {
                        index: *index,
                        indented: false,
                        last_child: false,
                    },
                    rows,
                    WorkspaceRowRenderOptions {
                        endpoint_active: true,
                        selected,
                        dragged,
                        first_row_reserved_width: reserved,
                        tick: state.tick,
                    },
                    palette,
                );
                if !dots.is_empty() {
                    render_folders_pane_dots(
                        buffer,
                        rect,
                        &dots,
                        config.status_indicators,
                        config.idle_attention_seconds,
                        workspace.focused,
                        state.tick,
                        palette,
                    );
                }
                hits.workspaces.push(WorkspaceHit {
                    rect,
                    endpoint_id: ClientEndpointId::Local,
                    workspace_id: workspace.workspace_id.clone(),
                    indented: false,
                    group_toggle: None,
                });
            }
        }
        let gap = gaps.get(position).copied().unwrap_or(0);
        y = y.saturating_add(row_height + gap);
    }
    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.workspace_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }
}
