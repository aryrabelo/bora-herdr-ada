use ratatui::layout::Rect;

use crate::app::state::{AppState, ProjectRowHitArea, ProjectRowTarget, ViewLayout};

use super::ScrollbarClickTarget;

impl AppState {
    pub(super) fn workspace_list_rect(&self) -> Rect {
        let sidebar = self.view.sidebar_rect;
        if self.sidebar_collapsed || sidebar.width <= 1 || sidebar.height == 0 {
            return Rect::default();
        }
        crate::ui::workspace_list_rect(sidebar, self.sidebar_section_split)
    }

    pub(super) fn agent_panel_rect(&self) -> Rect {
        let sidebar = self.view.sidebar_rect;
        if self.sidebar_collapsed || sidebar.width <= 1 || sidebar.height == 0 {
            return Rect::default();
        }
        let (_, detail_area) =
            crate::ui::expanded_sidebar_sections(sidebar, self.sidebar_section_split);
        detail_area
    }

    /// Resolve a COMMANDS band row (bora-55c.3) into a dispatchable
    /// `PendingBoraCommand` from the tick-refreshed command cache,
    /// including `$BORA_PORT` resolution — fresh at click time, which is an
    /// action, not render.
    pub(crate) fn section_command_launch(
        &self,
        ws_idx: usize,
        label: &str,
    ) -> Option<crate::app::state::PendingBoraCommand> {
        let ws = self.workspaces.get(ws_idx)?;
        let cmd = ws
            .cached_commands
            .as_deref()?
            .iter()
            .find(|c| c.label == label)?
            .clone();
        let branch = ws.cached_git_branch.as_deref();
        let checkout_path = ws
            .worktree_space()
            .map(|s| s.checkout_path.as_path())
            .unwrap_or(&ws.identity_cwd);
        let key = branch.map(str::to_string).unwrap_or_else(|| {
            checkout_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let port = ws
            .bora_config_root()
            .and_then(|root| crate::bora_settings::resolve_port(root, checkout_path, &key));
        Some(crate::app::state::PendingBoraCommand {
            ws_idx,
            command: cmd.command,
            mode: cmd.mode,
            label: Some(cmd.label),
            port,
        })
    }

    pub(super) fn workspace_list_scrollbar_target_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<ScrollbarClickTarget> {
        let area = self.workspace_list_rect();
        let metrics = crate::ui::workspace_list_scroll_metrics(self, area);
        let track = crate::ui::workspace_list_scrollbar_rect(self, area)?;
        if col < track.x
            || col >= track.x + track.width
            || row < track.y
            || row >= track.y + track.height
        {
            return None;
        }
        if let Some(grab_row_offset) = crate::ui::scrollbar_thumb_grab_offset(metrics, track, row) {
            Some(ScrollbarClickTarget::Thumb { grab_row_offset })
        } else {
            Some(ScrollbarClickTarget::Track {
                offset_from_bottom: crate::ui::scrollbar_offset_from_row(metrics, track, row),
            })
        }
    }

    pub(super) fn workspace_list_offset_for_drag_row(
        &self,
        row: u16,
        grab_row_offset: u16,
    ) -> Option<usize> {
        let area = self.workspace_list_rect();
        let metrics = crate::ui::workspace_list_scroll_metrics(self, area);
        let track = crate::ui::workspace_list_scrollbar_rect(self, area)?;
        Some(crate::ui::scrollbar_offset_from_drag_row(
            metrics,
            track,
            row,
            grab_row_offset,
        ))
    }

    pub(super) fn set_workspace_list_offset_from_bottom(&mut self, offset_from_bottom: usize) {
        let area = self.workspace_list_rect();
        let metrics = crate::ui::workspace_list_scroll_metrics(self, area);
        self.workspace_scroll = metrics
            .max_offset_from_bottom
            .saturating_sub(offset_from_bottom);
        self.workspace_scroll = crate::ui::normalized_workspace_scroll(
            self,
            self.view.sidebar_rect,
            self.workspace_scroll,
        );
    }

    pub(super) fn scroll_workspace_list(&mut self, delta: i16) {
        if delta.is_negative() {
            self.workspace_scroll = self
                .workspace_scroll
                .saturating_sub(delta.unsigned_abs() as usize);
            self.workspace_scroll = crate::ui::normalized_workspace_scroll(
                self,
                self.view.sidebar_rect,
                self.workspace_scroll,
            );
            return;
        }

        let area = self.workspace_list_rect();
        let metrics = crate::ui::workspace_list_scroll_metrics(self, area);
        self.workspace_scroll = self
            .workspace_scroll
            .saturating_add(delta as usize)
            .min(metrics.max_offset_from_bottom);
        self.workspace_scroll = crate::ui::normalized_workspace_scroll(
            self,
            self.view.sidebar_rect,
            self.workspace_scroll,
        );
    }

    pub(super) fn agent_panel_scrollbar_target_at(
        &self,
        col: u16,
        row: u16,
    ) -> Option<ScrollbarClickTarget> {
        let area = self.agent_panel_rect();
        let metrics = crate::ui::agent_panel_scroll_metrics(self, area);
        let track = crate::ui::agent_panel_scrollbar_rect(self, area)?;
        if col < track.x
            || col >= track.x + track.width
            || row < track.y
            || row >= track.y + track.height
        {
            return None;
        }
        if let Some(grab_row_offset) = crate::ui::scrollbar_thumb_grab_offset(metrics, track, row) {
            Some(ScrollbarClickTarget::Thumb { grab_row_offset })
        } else {
            Some(ScrollbarClickTarget::Track {
                offset_from_bottom: crate::ui::scrollbar_offset_from_row(metrics, track, row),
            })
        }
    }

    pub(super) fn agent_panel_offset_for_drag_row(
        &self,
        row: u16,
        grab_row_offset: u16,
    ) -> Option<usize> {
        let area = self.agent_panel_rect();
        let metrics = crate::ui::agent_panel_scroll_metrics(self, area);
        let track = crate::ui::agent_panel_scrollbar_rect(self, area)?;
        Some(crate::ui::scrollbar_offset_from_drag_row(
            metrics,
            track,
            row,
            grab_row_offset,
        ))
    }

    pub(super) fn set_agent_panel_offset_from_bottom(&mut self, offset_from_bottom: usize) {
        let area = self.agent_panel_rect();
        let metrics = crate::ui::agent_panel_scroll_metrics(self, area);
        self.agent_panel_scroll = metrics
            .max_offset_from_bottom
            .saturating_sub(offset_from_bottom);
    }

    pub(super) fn scroll_agent_panel(&mut self, delta: i16) {
        let area = self.agent_panel_rect();
        let max_scroll = crate::ui::agent_panel_scroll_metrics(self, area).max_offset_from_bottom;
        if delta.is_negative() {
            self.agent_panel_scroll = self
                .agent_panel_scroll
                .saturating_sub(delta.unsigned_abs() as usize);
        } else {
            self.agent_panel_scroll = self
                .agent_panel_scroll
                .saturating_add(delta as usize)
                .min(max_scroll);
        }
    }

    pub(crate) fn sidebar_footer_rect(&self) -> Rect {
        let ws_area = self.workspace_list_rect();
        if ws_area == Rect::default() {
            return Rect::default();
        }
        let y = ws_area.y + ws_area.height.saturating_sub(1);
        Rect::new(ws_area.x, y, ws_area.width, 1)
    }

    pub(crate) fn sidebar_new_button_rect(&self) -> Rect {
        let footer = self.sidebar_footer_rect();
        let width = 5u16.min(footer.width.max(1));
        Rect::new(footer.x, footer.y, width, footer.height)
    }

    pub(crate) fn global_launcher_rect(&self) -> Rect {
        if self.view.layout == ViewLayout::Mobile {
            return self.view.mobile_menu_hit_area;
        }

        let footer = self.sidebar_footer_rect();
        let width = if self.global_menu_attention_badge_visible() {
            8
        } else {
            6
        }
        .min(footer.width.max(1));
        // Since bora-49p.6 retired the agent panel, the workspace list runs to
        // the sidebar's last row — the same row as the collapse toggle. Stop
        // the right-aligned launcher before that cell, or it covers the toggle
        // and, because the launcher is hit-tested first, makes it unclickable.
        // Rendering reads this same rect, so both move together.
        let toggle = crate::ui::expanded_sidebar_toggle_rect(self.view.sidebar_rect);
        let mut right_edge = footer.x + footer.width;
        if toggle.width > 0 && toggle.y == footer.y && toggle.x < right_edge {
            right_edge = toggle.x;
        }
        let x = right_edge.saturating_sub(width).max(footer.x);
        Rect::new(x, footer.y, width, footer.height)
    }

    pub(crate) fn global_menu_labels(&self) -> Vec<&'static str> {
        let mut labels = vec!["settings"];
        if self.chat_view {
            labels.push("chat");
        }
        labels.push("keybinds");
        labels.push("reload config");
        if self.update_available.is_some() {
            labels.push("update ready");
        } else if self.latest_release_notes_available {
            labels.push("what's new");
        }
        labels.push("detach");
        labels
    }

    pub(crate) fn global_menu_rect(&self) -> Rect {
        let screen = self.screen_rect();
        let launcher = self.global_launcher_rect();
        let labels = self.global_menu_labels();
        let content_width = labels
            .iter()
            .map(|label| {
                let badge_width = if self.global_menu_item_has_badge(label) {
                    2
                } else {
                    0
                };
                label.chars().count() as u16 + badge_width
            })
            .max()
            .unwrap_or(8)
            .saturating_add(2);
        let menu_w = content_width.saturating_add(2).min(screen.width.max(1));
        let menu_h = (labels.len() as u16 + 2).min(screen.height.max(1));
        let max_x = screen.x + screen.width.saturating_sub(menu_w);
        let desired_x = launcher.x + launcher.width.saturating_sub(menu_w);
        let x = desired_x.min(max_x);
        let y = launcher.y.saturating_sub(menu_h);
        Rect::new(x, y, menu_w, menu_h)
    }

    pub(super) fn on_sidebar_divider(&self, col: u16, row: u16) -> bool {
        if self.sidebar_collapsed {
            return false;
        }
        let sidebar = self.view.sidebar_rect;
        let toggle = crate::ui::expanded_sidebar_toggle_rect(sidebar);
        let on_toggle = toggle.width > 0
            && col >= toggle.x
            && col < toggle.x + toggle.width
            && row >= toggle.y
            && row < toggle.y + toggle.height;
        sidebar.width > 0
            && !on_toggle
            && col == sidebar.x + sidebar.width.saturating_sub(1)
            && row >= sidebar.y
            && row < sidebar.y + sidebar.height
    }

    pub(super) fn on_sidebar_toggle(&self, col: u16, row: u16) -> bool {
        let rect = if self.sidebar_collapsed {
            crate::ui::collapsed_sidebar_toggle_rect(self.view.sidebar_rect)
        } else {
            crate::ui::expanded_sidebar_toggle_rect(self.view.sidebar_rect)
        };
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    pub(super) fn set_manual_sidebar_width(&mut self, divider_col: u16) {
        let sidebar = self.view.sidebar_rect;
        let width = divider_col.saturating_sub(sidebar.x).saturating_add(1);
        self.sidebar_width = width.clamp(self.sidebar_min_width, self.sidebar_max_width);
        self.sidebar_width_source = crate::app::state::SidebarWidthSource::Manual;
        self.mark_session_dirty();
    }

    pub(super) fn on_sidebar_section_divider(&self, col: u16, row: u16) -> bool {
        if self.sidebar_collapsed {
            return false;
        }
        let rect = crate::ui::sidebar_section_divider_rect(
            self.view.sidebar_rect,
            self.sidebar_section_split,
        );
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    pub(super) fn on_right_panel_divider(&self, col: u16, _row: u16) -> bool {
        if self.right_panel_collapsed {
            return false;
        }
        let rect = self.view.right_panel_rect;
        rect.width > 0 && col == rect.x
    }

    pub(super) fn on_right_panel_toggle(&self, col: u16, row: u16) -> bool {
        let rect = if self.right_panel_collapsed {
            crate::ui::right_panel::collapsed_right_panel_toggle_rect(self.view.terminal_area)
        } else {
            crate::ui::right_panel::expanded_right_panel_toggle_rect(self.view.right_panel_rect)
        };
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    /// Map a screen row in the right panel Changes tab to a `(ChangeSectionKind, file_path)`.
    ///
    /// Returns `None` if the row is a section header, out of range, or there's no change set.
    pub(super) fn right_panel_file_at_row(
        &self,
        screen_row: u16,
    ) -> Option<(crate::workspace::ChangeSectionKind, String)> {
        let rp = self.view.right_panel_rect;
        // Body starts after separator column (row rp.y) + tab header (row rp.y+1, content row 0)
        // so body row 0 is at screen rp.y + 1 (the tab header occupies the first content row)
        // But the content area is rp.y with the separator drawn at rp.x, tab header at rp.y,
        // body at rp.y + 1. The click is on screen_row, body starts at rp.y + 1.
        let body_start = rp.y + 1; // tab header is row rp.y
        if screen_row < body_start {
            return None;
        }
        let row_in_body = (screen_row - body_start) as usize;
        let scroll = self.right_panel_scroll as usize;
        let flat_index = row_in_body + scroll;

        let ws = self.active.and_then(|i| self.workspaces.get(i))?;
        let cs = ws.cached_change_set.as_ref()?;

        // Walk the same flat layout as render_changes_tab
        let mut idx = 0;
        for section in &cs.sections {
            idx += 1; // section header line
            for file in &section.files {
                if idx == flat_index {
                    return Some((section.kind.clone(), file.path.clone()));
                }
                idx += 1;
            }
        }
        None
    }

    /// Map a screen row in the right panel Issues tab to `(issue number, url)`.
    ///
    /// Walks the same flat layout as `render_issues_tab` (one row per issue,
    /// offset by `right_panel_scroll`). Returns `None` when the row is out of
    /// range, the cache is missing/errored, or the workspace has no repo.
    pub(super) fn right_panel_issue_at_row(&self, screen_row: u16) -> Option<(u64, String)> {
        let rp = self.view.right_panel_rect;
        let body_start = rp.y + 1; // tab header is row rp.y
        if screen_row < body_start {
            return None;
        }
        let row_in_body = (screen_row - body_start) as usize;
        let flat_index = row_in_body + self.right_panel_scroll as usize;

        let ws = self.active.and_then(|i| self.workspaces.get(i))?;
        let repo_identity = ws.git_space().map(|space| space.repo_identity.clone())?;
        let cache = self.repo_issues.get(&repo_identity)?;
        if cache.error.is_some() {
            return None;
        }
        let issue = cache.issues.get(flat_index)?;
        Some((issue.number, issue.url.clone()))
    }

    /// Map a screen row in the right panel PRs tab to `(number, url, head_ref)`.
    ///
    /// Walks the same flat layout as `render_prs_tab` (one row per PR, offset
    /// by `right_panel_scroll`). Returns `None` when the row is out of range,
    /// the cache is missing/errored, or the workspace has no repo.
    pub(super) fn right_panel_pr_at_row(&self, screen_row: u16) -> Option<(u64, String, String)> {
        let rp = self.view.right_panel_rect;
        let body_start = rp.y + 1; // tab header is row rp.y
        if screen_row < body_start {
            return None;
        }
        let row_in_body = (screen_row - body_start) as usize;
        let flat_index = row_in_body + self.right_panel_scroll as usize;

        let ws = self.active.and_then(|i| self.workspaces.get(i))?;
        let repo_identity = ws.git_space().map(|space| space.repo_identity.clone())?;
        let cache = self.repo_open_prs.get(&repo_identity)?;
        if cache.error.is_some() {
            return None;
        }
        let pr = cache.prs.get(flat_index)?;
        Some((pr.number, pr.url.clone(), pr.head_ref_name.clone()))
    }

    pub(super) fn set_sidebar_section_split(&mut self, row: u16) {
        let sidebar = self.view.sidebar_rect;
        let content_height = sidebar.height;
        if content_height < 6 {
            return;
        }
        let relative_y = row.saturating_sub(sidebar.y);
        let ratio = f32::from(relative_y) / f32::from(content_height);
        self.sidebar_section_split = ratio.clamp(0.1, 0.9);
        self.mark_session_dirty();
    }

    pub(super) fn workspace_at_row(&self, row: u16) -> Option<usize> {
        let footer = self.sidebar_footer_rect();
        if footer == Rect::default() {
            return None;
        }

        let cards = if self.view.workspace_card_areas.is_empty() {
            crate::ui::compute_workspace_card_areas(self, self.view.sidebar_rect)
        } else {
            self.view.workspace_card_areas.clone()
        };

        cards.iter().find_map(|card| {
            (row >= card.rect.y && row < card.rect.y + card.rect.height).then_some(card.ws_idx)
        })
    }

    pub(super) fn collapsed_workspace_at_row(&self, row: u16) -> Option<usize> {
        if !self.sidebar_collapsed {
            return None;
        }

        let (ws_area, _, _) = crate::ui::collapsed_sidebar_sections(self.view.sidebar_rect);
        if ws_area == Rect::default() || row < ws_area.y || row >= ws_area.y + ws_area.height {
            return None;
        }

        let idx = (row - ws_area.y) as usize;
        (idx < self.workspaces.len()).then_some(idx)
    }

    pub(super) fn collapsed_agent_detail_target_at(
        &self,
        row: u16,
    ) -> Option<(usize, usize, crate::layout::PaneId)> {
        if !self.sidebar_collapsed {
            return None;
        }

        let (_, _, detail_area) = crate::ui::collapsed_sidebar_sections(self.view.sidebar_rect);
        let detail_content_area = Rect::new(
            detail_area.x,
            detail_area.y,
            detail_area.width,
            detail_area.height.saturating_sub(1),
        );
        if detail_content_area == Rect::default()
            || row < detail_content_area.y
            || row >= detail_content_area.y + detail_content_area.height
        {
            return None;
        }

        let detail_idx = (row - detail_content_area.y) as usize;
        let details = crate::ui::agent_panel_entries(self);
        let detail = details.get(detail_idx)?;
        Some((detail.ws_idx, detail.tab_idx, detail.pane_id))
    }

    pub(super) fn workspace_drop_index_at_row(&self, row: u16) -> Option<usize> {
        let area = self.workspace_list_rect();
        let footer = self.sidebar_footer_rect();
        if area == Rect::default() || row < area.y || row >= footer.y {
            return None;
        }

        let cards = if self.view.workspace_card_areas.is_empty() {
            crate::ui::compute_workspace_card_areas(self, self.view.sidebar_rect)
        } else {
            self.view.workspace_card_areas.clone()
        };
        if cards.is_empty() {
            return Some(0);
        }

        let mut insert_indices = Vec::with_capacity(cards.len() + 1);
        for (idx, card) in cards.iter().enumerate() {
            let card_group = self
                .workspaces
                .get(card.ws_idx)
                .and_then(|ws| ws.worktree_space())
                .map(|space| space.key.as_str());
            let previous_group = idx.checked_sub(1).and_then(|prev_idx| {
                self.workspaces
                    .get(cards[prev_idx].ws_idx)
                    .and_then(|ws| ws.worktree_space())
                    .map(|space| space.key.as_str())
            });
            // Repo view owns this suppression: its linked worktrees render
            // NESTED under the group root, so gaps between same-repo
            // siblings are not independent slots. Project and Folders make
            // every row a top-level root, so every gap there is a real
            // slot — suppressing them made drops land on the wrong side of
            // same-repo siblings.
            let inside_group_gap = self.view_mode == crate::config::ViewMode::Repo
                && card_group.is_some()
                && card_group == previous_group;
            if !inside_group_gap {
                insert_indices.push(card.ws_idx);
            }
        }
        insert_indices.push(cards.last().map(|card| card.ws_idx + 1).unwrap_or(0));

        let mut best: Option<(usize, u16)> = None;
        for insert_idx in insert_indices {
            let Some(slot_row) = crate::ui::workspace_drop_indicator_row(&cards, area, insert_idx)
            else {
                continue;
            };
            let distance = row.abs_diff(slot_row);
            match best {
                Some((best_idx, best_distance))
                    if distance > best_distance
                        || (distance == best_distance && insert_idx < best_idx) => {}
                _ => best = Some((insert_idx, distance)),
            }
        }

        best.map(|(insert_idx, _)| insert_idx)
    }

    pub(super) fn workspace_move_block_params(
        &self,
        source_ws_idx: usize,
        insert_idx: usize,
    ) -> Option<crate::api::schema::WorkspaceMoveBlockParams> {
        let source = self.workspaces.get(source_ws_idx)?;
        // A linked worktree is not a drag root in Flat/Repo view: it renders
        // as an indented child under its main checkout, so reordering it
        // alone is meaningless. Project view inverts that — every workspace
        // renders its own top-level `PaneDotsRow` block (6a), which the
        // `roots` filter below treats as a root. Applying this guard
        // there let the drag OPEN and then silently swallowed the drop, since
        // `None` here means "nothing to move". Folders view (2026-08-31) is
        // the same shape: every row is a top-level `PaneDotsRow`, so it
        // gets the same exemption.
        if !matches!(
            self.view_mode,
            crate::config::ViewMode::Project | crate::config::ViewMode::Folders
        ) && source
            .worktree_space()
            .is_some_and(|space| space.is_linked_worktree)
        {
            return None;
        }

        let roots = crate::ui::workspace_list_entries_expanded(self)
            .into_iter()
            .filter_map(|entry| match entry {
                crate::ui::WorkspaceListEntry::Workspace {
                    ws_idx,
                    indented: false,
                    ..
                } => Some(ws_idx),
                crate::ui::WorkspaceListEntry::Workspace { .. } => None,
                crate::ui::WorkspaceListEntry::GroupHeader { .. }
                | crate::ui::WorkspaceListEntry::ProjectHeader { .. }
                | crate::ui::WorkspaceListEntry::BranchHeader { .. }
                | crate::ui::WorkspaceListEntry::HiddenHeader { .. } => None,
                // Project view (6a): every member workspace is its own
                // `PaneDotsRow` block, and the BLOCK is the drag root —
                // one full section per branch GROUP now, so the group's
                // `SectionRow` names only the representative and is a
                // header, not a workspace identity to reorder. The drag
                // STARTS on the block anyway (P2, bora-79l T1: the
                // block's `WorkspaceCardArea` feeds
                // `workspace_presses`); roots and affordance finally
                // name the same row.
                crate::ui::WorkspaceListEntry::PaneDotsRow { ws_idx, .. } => Some(ws_idx),
                crate::ui::WorkspaceListEntry::ProjectRow { .. }
                | crate::ui::WorkspaceListEntry::WorktreeRow { .. }
                | crate::ui::WorkspaceListEntry::SectionHeader { .. }
                | crate::ui::WorkspaceListEntry::SectionItem { .. }
                | crate::ui::WorkspaceListEntry::PrRow { .. }
                | crate::ui::WorkspaceListEntry::SectionRow { .. } => None,
            })
            .collect::<Vec<_>>();
        let source_pos = roots.iter().position(|ws_idx| *ws_idx == source_ws_idx)?;
        let remaining_roots = roots
            .iter()
            .copied()
            .filter(|ws_idx| *ws_idx != source_ws_idx)
            .collect::<Vec<_>>();
        // A group parent dragged to the bottom yields an insert_idx of
        // `last_card.ws_idx + 1`, which can collide with a member of the source's
        // own (moving) group. Treat any insert target that belongs to the source
        // block as "end", so the whole block lands after the remaining roots.
        // Folders has no blocks: it moves ONE row per drag — its contract is
        // the Flat view's — so no group key and no block-end special case.
        let source_group_key = if self.view_mode == crate::config::ViewMode::Folders {
            None
        } else {
            source.worktree_space().map(|space| space.key.clone())
        };
        let target_in_source_block = self
            .workspaces
            .get(insert_idx)
            .and_then(|target| target.worktree_space())
            .zip(source_group_key.as_ref())
            .is_some_and(|(target_space, key)| target_space.key == *key);
        let effective_end = self.workspaces.get(insert_idx).is_none() || target_in_source_block;
        let insert_pos = if effective_end {
            remaining_roots.len()
        } else {
            remaining_roots
                .iter()
                .position(|ws_idx| *ws_idx == insert_idx)?
        };
        if insert_pos == source_pos {
            return None;
        }

        let workspace_ids = if self.view_mode == crate::config::ViewMode::Folders {
            // Folders: the dragged row travels ALONE — never its repo
            // siblings, which are independent rows in this view.
            vec![source.id.clone()]
        } else {
            match source.worktree_space() {
                Some(source_space) => {
                    let mut ids = vec![source.id.clone()];
                    ids.extend(
                        self.workspaces
                            .iter()
                            .filter(|workspace| workspace.id != source.id)
                            .filter(|workspace| {
                                workspace
                                    .worktree_space()
                                    .is_some_and(|space| space.key == source_space.key)
                            })
                            .map(|workspace| workspace.id.clone()),
                    );
                    ids
                }
                None => vec![source.id.clone()],
            }
        };
        let before_workspace_id = if effective_end {
            None
        } else {
            match self.workspaces.get(insert_idx) {
                Some(target) => {
                    let anchor = match crate::ui::workspace_parent_group_state(self, insert_idx)
                        .and_then(|_| target.worktree_space())
                    {
                        Some(target_space) => self
                            .workspaces
                            .iter()
                            .find(|workspace| {
                                workspace
                                    .worktree_space()
                                    .is_some_and(|space| space.key == target_space.key)
                            })
                            .unwrap_or(target),
                        None => target,
                    };
                    Some(anchor.id.clone())
                }
                None => None,
            }
        };

        Some(crate::api::schema::WorkspaceMoveBlockParams {
            workspace_ids,
            before_workspace_id,
        })
    }

    /// Hit test for the sidebar's view-mode cycle target: the current view's
    /// name, right-aligned on the workspace list's first row.
    ///
    /// Removed in 7bb8133b together with the ` spaces` title it shared that
    /// row with, and restored on its own: the owner wanted the title gone,
    /// not the only mouse affordance for cycling Flat/Repo/Project. The rect
    /// claims just the trailing `label.len()` cells, so the rest of the row
    /// still serves the drag "drop above the first card" indicator.
    pub(super) fn on_view_mode_toggle(&self, col: u16, row: u16) -> bool {
        if self.sidebar_collapsed {
            return false;
        }

        let (ws_area, _) = crate::ui::expanded_sidebar_sections(
            self.view.sidebar_rect,
            self.sidebar_section_split,
        );
        let rect = crate::ui::view_mode_toggle_rect(ws_area, self.view_mode);
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    pub(super) fn on_agent_panel_sort_toggle(&self, col: u16, row: u16) -> bool {
        if self.sidebar_collapsed || self.agent_view_override.is_some() {
            return false;
        }

        let (_, detail_area) = crate::ui::expanded_sidebar_sections(
            self.view.sidebar_rect,
            self.sidebar_section_split,
        );
        let rect = crate::ui::agent_panel_toggle_rect(detail_area, self.agent_panel_sort);
        rect.width > 0
            && col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
    }

    pub(super) fn agent_detail_target_at(
        &self,
        row: u16,
    ) -> Option<(usize, usize, crate::layout::PaneId)> {
        if self.sidebar_collapsed {
            return None;
        }

        let detail_area = self.agent_panel_rect();
        let metrics = crate::ui::agent_panel_scroll_metrics(self, detail_area);
        let body = crate::ui::agent_panel_body_rect(
            detail_area,
            crate::ui::should_show_scrollbar(metrics),
        );
        if body.height == 0 || row < body.y || row >= body.y + body.height {
            return None;
        }

        let mut row_y = body.y;
        let body_bottom = body.y + body.height;
        let entries = crate::ui::agent_panel_entries(self);
        let scroll = self.agent_panel_scroll.min(metrics.max_offset_from_bottom);
        for (index, detail) in entries.iter().enumerate().skip(scroll) {
            let height = crate::ui::agent_entry_height_in_body(self, detail, body.height);
            if row_y.saturating_add(height) > body_bottom {
                break;
            }
            if row >= row_y && row < row_y.saturating_add(height) {
                return Some((detail.ws_idx, detail.tab_idx, detail.pane_id));
            }
            row_y = row_y
                .saturating_add(height)
                .saturating_add(crate::ui::agent_entry_gap(self, index, entries.len()))
                .min(body_bottom);
        }
        None
    }
}

/// Resolves a sidebar click to the Project-view row it landed on, reading
/// only the geometry pass's own `ProjectRowHitArea`s
/// (`ViewState.project_row_areas`) — never the mouse row on its own.
/// `ProjectRowHitArea::rect` is the single source of truth for where a row
/// sits; a caller that instead derived a row index from `y` and indexed
/// into a list would silently land on the wrong target the moment any row
/// above it collapses, expands, or scrolls. `areas` need not start at
/// `y == 0` — the scrolled/offset case a collapsed row above produces.
pub(super) fn project_row_target_at(
    areas: &[ProjectRowHitArea],
    x: u16,
    y: u16,
) -> Option<&ProjectRowTarget> {
    areas
        .iter()
        .find(|area| {
            x >= area.rect.x
                && x < area.rect.x + area.rect.width
                && y >= area.rect.y
                && y < area.rect.y + area.rect.height
        })
        .map(|area| &area.target)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crossterm::event::{MouseButton, MouseEventKind};
    use ratatui::layout::Rect;

    use super::super::{app_for_mouse_test, capture_snapshot, mouse, unique_temp_path};
    use crate::{
        app::state::{AgentPanelSort, DragTarget, Mode, ProjectRowHitArea, ProjectRowTarget},
        config::SidebarCollapsedModeConfig,
        detect::{Agent, AgentState},
        workspace::Workspace,
    };

    #[test]
    fn project_row_target_at_resolves_against_offset_area_geometry_not_zero_based_rows() {
        // Areas start at y=7, not y=0 — the epic's own adversarial case: a
        // lookup that assumes row N sits at `areas[N]` or derives position
        // from `y - 0` must fail this test the moment collapsing something
        // above pushes these rows down.
        let areas = vec![
            ProjectRowHitArea {
                rect: Rect::new(2, 7, 20, 1),
                target: ProjectRowTarget::Project {
                    collapse_key: "project:cnb".into(),
                },
            },
            ProjectRowHitArea {
                rect: Rect::new(2, 8, 20, 1),
                target: ProjectRowTarget::Section {
                    ws_idx: 7,
                    checkout_key: "cnb-main".into(),
                    collapse_key: "wsec:7".into(),
                },
            },
            ProjectRowHitArea {
                rect: Rect::new(2, 12, 20, 3),
                target: ProjectRowTarget::Band {
                    collapse_key: "section:cnb:main:checks".into(),
                },
            },
        ];

        assert_eq!(
            super::project_row_target_at(&areas, 5, 7),
            Some(&ProjectRowTarget::Project {
                collapse_key: "project:cnb".into()
            }),
        );
        assert_eq!(
            super::project_row_target_at(&areas, 5, 8),
            Some(&ProjectRowTarget::Section {
                ws_idx: 7,
                checkout_key: "cnb-main".into(),
                collapse_key: "wsec:7".into(),
            }),
        );
        // First and last row of the taller (height-3) rect both resolve to
        // the same target, not just its top row.
        assert_eq!(
            super::project_row_target_at(&areas, 5, 12),
            Some(&ProjectRowTarget::Band {
                collapse_key: "section:cnb:main:checks".into()
            }),
        );
        assert_eq!(
            super::project_row_target_at(&areas, 5, 14),
            Some(&ProjectRowTarget::Band {
                collapse_key: "section:cnb:main:checks".into()
            }),
        );
    }

    #[test]
    fn project_row_target_at_returns_none_outside_every_area() {
        let areas = vec![ProjectRowHitArea {
            rect: Rect::new(2, 7, 20, 1),
            target: ProjectRowTarget::Project {
                collapse_key: "project:cnb".into(),
            },
        }];

        assert_eq!(
            super::project_row_target_at(&areas, 5, 6),
            None,
            "row above"
        );
        assert_eq!(
            super::project_row_target_at(&areas, 5, 8),
            None,
            "row below"
        );
        assert_eq!(
            super::project_row_target_at(&areas, 1, 7),
            None,
            "col left of rect"
        );
        assert_eq!(
            super::project_row_target_at(&areas, 22, 7),
            None,
            "col at x + width"
        );
    }

    #[test]
    fn clicking_launcher_opens_global_menu() {
        let mut app = app_for_mouse_test();
        let rect = app.state.global_launcher_rect();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            rect.x + rect.width.saturating_sub(1),
            rect.y,
        ));

        assert_eq!(app.state.mode, Mode::GlobalMenu);
    }

    #[test]
    fn hovering_global_menu_updates_highlight() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(MouseEventKind::Moved, menu.x + 2, menu.y + 2));

        assert_eq!(app.state.global_menu.highlighted, 1);
    }

    #[test]
    fn clicking_keybinds_menu_item_opens_help() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 2,
        ));

        assert_eq!(app.state.mode, Mode::KeybindHelp);
    }

    #[test]
    fn clicking_settings_menu_item_opens_settings() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 1,
        ));

        assert_eq!(app.state.mode, Mode::Settings);
    }

    #[test]
    fn clicking_reload_config_menu_item_requests_reload() {
        let mut app = app_for_mouse_test();
        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 3,
        ));

        assert!(app.state.request_reload_config);
        assert_eq!(app.state.mode, Mode::Navigate);
    }

    #[test]
    fn update_pending_menu_surfaces_update_ready_entry() {
        let mut app = app_for_mouse_test();
        app.state.update_available = Some("0.3.2".into());
        app.state.latest_release_notes_available = true;

        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        assert_eq!(
            app.state.global_menu_labels(),
            vec![
                "settings",
                "keybinds",
                "reload config",
                "update ready",
                "detach"
            ]
        );
        assert!(!app.state.should_quit);
    }

    #[test]
    fn persistence_mode_menu_surfaces_detach_action() {
        let mut app = app_for_mouse_test();
        app.state.detach_exits = false;

        let launcher = app.state.global_launcher_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            launcher.x,
            launcher.y,
        ));

        assert_eq!(
            app.state.global_menu_labels(),
            vec!["settings", "keybinds", "reload config", "detach"]
        );

        let menu = app.state.global_menu_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 4,
        ));

        assert!(app.state.detach_requested);
        assert!(!app.state.should_quit);
        assert_ne!(app.state.mode, Mode::GlobalMenu);
    }

    #[test]
    fn whats_new_remains_in_menu_for_latest_installed_release_notes() {
        let mut app = app_for_mouse_test();
        app.state.latest_release_notes_available = true;

        assert_eq!(
            app.state.global_menu_labels(),
            vec![
                "settings",
                "keybinds",
                "reload config",
                "what's new",
                "detach"
            ]
        );
    }

    /// The agent panel is retired from the live layout (bora-49p.6): the
    /// sidebar hands its whole column to the workspace list, so the panel
    /// occupies no rows and can be clicked on none of them.
    ///
    /// This replaces a test that asserted the panel's row packing through
    /// `agent_detail_target_at`. That lookup derives its body from the live
    /// `agent_panel_rect()`, so preserving it would have meant feeding it a
    /// fabricated rect — asserting that clicks land on a panel the user
    /// cannot see. The packing math itself is pure and still covered by the
    /// `crate::ui::sidebar` tests. What is worth guarding here is the
    /// retirement: if the panel silently returns, this fails.
    #[test]
    fn retired_agent_panel_claims_no_rows_and_answers_no_mouse_target() {
        let mut app = app_for_mouse_test();
        let first = Workspace::test_new("one");
        let first_pane = first.tabs[0].root_pane;
        let second = Workspace::test_new("two");
        let second_pane = second.tabs[0].root_pane;
        app.state.workspaces = vec![first, second];
        app.state.ensure_test_terminals();
        for (ws_idx, pane_id, agent) in
            [(0, first_pane, Agent::Pi), (1, second_pane, Agent::Claude)]
        {
            let terminal_id = app.state.workspaces[ws_idx].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            app.state
                .terminals
                .get_mut(&terminal_id)
                .unwrap()
                .detected_agent = Some(agent);
        }

        // Two agents exist, so the panel would have had entries to show.
        assert_eq!(app.state.agent_panel_rect().height, 0);

        let sidebar = app.state.view.sidebar_rect;
        for row in sidebar.y..sidebar.y + sidebar.height {
            assert_eq!(
                app.state.agent_detail_target_at(row),
                None,
                "row {row} must not resolve to an agent-panel target"
            );
        }

        // The workspace list, meanwhile, got the column the panel gave up.
        let ws_rect = app.state.workspace_list_rect();
        assert_eq!(ws_rect.height, sidebar.height);

        // This also replaces five deleted tests that each clicked one panel
        // affordance (a detail row, a scrolled detail row, the sort toggle, an
        // all-workspaces row, and the post-filter scroll clamp). Retired, none
        // of those clicks can land, so five variants of "nothing happens" are
        // one fact: no click anywhere in the sidebar touches panel state. The
        // sweep stops above the footer row: the launcher ("new") is workspace
        // territory, not panel territory — before bora-55c.3 the Programs
        // band's prompt row happened to open a modal that swallowed it.
        let sort_before = app.state.agent_panel_sort;
        let active_before = app.state.active;
        let tab_before = app.state.workspaces[0].active_tab;
        for row in sidebar.y..sidebar.y + sidebar.height - 1 {
            app.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                sidebar.x + 1,
                row,
            ));
        }
        assert_eq!(app.state.agent_panel_sort, sort_before);
        assert_eq!(app.state.agent_panel_scroll, 0);
        assert_eq!(app.state.workspaces[0].active_tab, tab_before);
        assert_eq!(app.state.active, active_before);
    }

    #[test]
    fn wheel_over_the_sidebar_never_scrolls_the_retired_agent_panel() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let first_pane = ws.tabs[0].root_pane;

        let mut tabs = Vec::new();
        for (tab_name, agent) in [
            ("logs", Agent::Claude),
            ("review", Agent::Codex),
            ("ops", Agent::Gemini),
        ] {
            let tab_idx = ws.test_add_tab(Some(tab_name));
            let pane_id = ws.tabs[tab_idx].root_pane;
            tabs.push((tab_idx, pane_id, agent));
        }

        app.state.workspaces = vec![ws];
        app.state.ensure_test_terminals();
        let first_terminal_id = app.state.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Pi);
        for (tab_idx, pane_id, agent) in tabs {
            let terminal_id = app.state.workspaces[0].tabs[tab_idx].panes[&pane_id]
                .attached_terminal_id
                .clone();
            app.state
                .terminals
                .get_mut(&terminal_id)
                .unwrap()
                .detected_agent = Some(agent);
        }
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        // Four agents across four tabs: pre-retirement this overflowed the
        // panel and the wheel scrolled it. Retired (bora-49p.6), the panel
        // owns no rows, so a wheel event anywhere in the sidebar reaches the
        // workspace list instead and never moves `agent_panel_scroll`.
        assert_eq!(app.state.agent_panel_rect().height, 0);
        let sidebar = app.state.view.sidebar_rect;

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            sidebar.x + 1,
            sidebar.y + sidebar.height - 2,
        ));

        assert_eq!(app.state.agent_panel_scroll, 0);
        assert_eq!(app.state.selected, 0);
    }

    #[test]
    fn clicking_collapsed_agent_row_switches_to_correct_tab_and_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let first_pane = ws.tabs[0].root_pane;
        let second_tab = ws.test_add_tab(Some("logs"));
        let second_pane = ws.tabs[second_tab].root_pane;
        app.state.workspaces = vec![ws];
        app.state.ensure_test_terminals();
        let first_terminal_id = app.state.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Pi);
        let second_terminal_id = app.state.workspaces[0].tabs[second_tab].panes[&second_pane]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&second_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Claude);
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.sidebar_collapsed = true;
        app.state.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        app.state.view.terminal_area = Rect::new(4, 0, 80, 20);

        let (_, _, detail_area) =
            crate::ui::collapsed_sidebar_sections(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            detail_area.x,
            detail_area.y + 1,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, 1);
        assert_eq!(
            app.state.workspaces[0].tabs[1].layout.focused(),
            second_pane
        );
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_collapsed_priority_agent_row_switches_to_matching_workspace() {
        let mut app = app_for_mouse_test();
        let first = Workspace::test_new("one");
        let first_pane = first.tabs[0].root_pane;
        let second = Workspace::test_new("two");
        let second_pane = second.tabs[0].root_pane;

        app.state.workspaces = vec![first, second];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.sidebar_collapsed = true;
        app.state.agent_panel_sort = AgentPanelSort::Priority;
        app.state.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        app.state.view.terminal_area = Rect::new(4, 0, 80, 20);

        let set_state = |app: &mut crate::app::App, ws_idx: usize, pane_id, state| {
            let terminal_id = app.state.workspaces[ws_idx].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = state;
        };
        set_state(&mut app, 0, first_pane, AgentState::Working);
        set_state(&mut app, 1, second_pane, AgentState::Blocked);

        let (_, _, detail_area) =
            crate::ui::collapsed_sidebar_sections(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            detail_area.x,
            detail_area.y,
        ));

        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.selected, 1);
        assert_eq!(
            app.state.workspaces[1].tabs[0].layout.focused(),
            second_pane
        );
    }

    #[test]
    fn clicking_collapsed_sidebar_toggle_expands_sidebar() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_collapsed = true;
        app.state.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        app.state.view.terminal_area = Rect::new(4, 0, 80, 20);

        let toggle = crate::ui::collapsed_sidebar_toggle_rect(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle.x,
            toggle.y,
        ));

        assert!(!app.state.sidebar_collapsed);
    }

    #[test]
    fn hidden_collapsed_sidebar_has_no_mouse_expand_hotspot() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_collapsed = true;
        app.state.sidebar_collapsed_mode = SidebarCollapsedModeConfig::Hidden;
        app.state.view.sidebar_rect = Rect::new(0, 0, 0, 20);
        app.state.view.terminal_area = Rect::new(0, 0, 80, 20);

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 19));

        assert!(app.state.sidebar_collapsed);
    }

    #[test]
    fn clicking_expanded_sidebar_toggle_collapses_sidebar() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_collapsed = false;
        app.state.view.sidebar_rect = Rect::new(0, 0, 26, 20);
        app.state.view.terminal_area = Rect::new(26, 0, 80, 20);

        let toggle = crate::ui::expanded_sidebar_toggle_rect(app.state.view.sidebar_rect);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle.x,
            toggle.y,
        ));

        assert!(app.state.sidebar_collapsed);
        assert!(app.state.drag.is_none());
    }

    /// The workspace list now runs to the sidebar's last row (bora-49p.6), so
    /// the right-aligned global launcher shares that row with the collapse
    /// toggle. The launcher is hit-tested first, so any overlap makes the
    /// toggle silently unclickable — which is exactly what happened before the
    /// launcher was taught to stop at the toggle's column.
    #[test]
    fn global_launcher_never_covers_the_sidebar_collapse_toggle() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_collapsed = false;
        app.state.view.sidebar_rect = Rect::new(0, 0, 26, 20);
        app.state.view.terminal_area = Rect::new(26, 0, 80, 20);

        let toggle = crate::ui::expanded_sidebar_toggle_rect(app.state.view.sidebar_rect);
        let launcher = app.state.global_launcher_rect();

        // They do share the bottom row now — the point is that they must not
        // share a cell.
        assert_eq!(launcher.y, toggle.y);
        assert!(
            launcher.x + launcher.width <= toggle.x,
            "launcher {launcher:?} must end before the toggle at {toggle:?}"
        );
    }

    #[test]
    fn clicking_workspace_switches_on_mouse_up() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        for ws in &mut app.state.workspaces {
            ws.cached_git_branch = None;
        }
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let target_row = app.state.view.workspace_card_areas[1].rect.y;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            target_row,
        ));
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspace_presses.len(), 1);

        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, target_row));
        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.selected, 1);
        assert!(app.state.workspace_presses.is_empty());
        let snapshot = capture_snapshot(&app.state);
        assert_eq!(snapshot.active, Some(1));
        assert_eq!(snapshot.selected, 1);
    }

    #[test]
    fn clicking_worktree_parent_row_focuses_workspace_without_toggling() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("main"), Workspace::test_new("issue")];
        for (idx, checkout_path) in ["/repo/herdr", "/repo/herdr-issue"].into_iter().enumerate() {
            app.state.workspaces[idx].worktree_space =
                Some(crate::workspace::WorktreeSpaceMembership {
                    key: "repo-key".into(),
                    label: "herdr".into(),
                    repo_root: "/repo/herdr".into(),
                    checkout_path: checkout_path.into(),
                    is_linked_worktree: idx > 0,
                });
            app.state.workspaces[idx].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
                key: "repo-key".into(),
                repo_identity: "repo-key".into(),
                checkout_key: checkout_path.to_string(),
                repo_name: "herdr".into(),
                repo_root: "/repo/herdr".into(),
                is_linked_worktree: idx > 0,
            });
        }
        app.state.active = None;
        app.state.mode = Mode::Terminal;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let parent = app.state.view.workspace_card_areas[0].rect;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            parent.x + 2,
            parent.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            parent.x + 2,
            parent.y,
        ));

        assert_eq!(app.state.active, Some(0));
        assert!(!app.state.collapsed_space_keys.contains("repo-key"));
    }

    #[test]
    fn clicking_worktree_parent_chevron_toggles_group_only() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("main"), Workspace::test_new("issue")];
        for (idx, checkout_path) in ["/repo/herdr", "/repo/herdr-issue"].into_iter().enumerate() {
            app.state.workspaces[idx].worktree_space =
                Some(crate::workspace::WorktreeSpaceMembership {
                    key: "repo-key".into(),
                    label: "herdr".into(),
                    repo_root: "/repo/herdr".into(),
                    checkout_path: checkout_path.into(),
                    is_linked_worktree: idx > 0,
                });
            app.state.workspaces[idx].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
                key: "repo-key".into(),
                repo_identity: "repo-key".into(),
                checkout_key: checkout_path.to_string(),
                repo_name: "herdr".into(),
                repo_root: "/repo/herdr".into(),
                is_linked_worktree: idx > 0,
            });
        }
        app.state.active = None;
        app.state.mode = Mode::Terminal;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let parent = app.state.view.workspace_card_areas[0];
        let chevron = crate::ui::workspace_group_chevron_rect(&parent);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            chevron.x,
            chevron.y,
        ));

        assert_eq!(app.state.active, None);
        assert!(app.state.workspace_presses.is_empty());
        assert!(app.state.collapsed_space_keys.contains("repo-key"));

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            chevron.x,
            chevron.y,
        ));

        assert!(!app.state.collapsed_space_keys.contains("repo-key"));
    }

    #[test]
    fn wheel_workspace_selection_follows_grouped_visual_order_without_scrollbar() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            Workspace::test_new("main"),
            Workspace::test_new("normal"),
            Workspace::test_new("issue"),
        ];
        for (idx, checkout_path) in [(0, "/repo/herdr"), (2, "/repo/herdr-issue")] {
            app.state.workspaces[idx].worktree_space =
                Some(crate::workspace::WorktreeSpaceMembership {
                    key: "repo-key".into(),
                    label: "herdr".into(),
                    repo_root: "/repo/herdr".into(),
                    checkout_path: checkout_path.into(),
                    is_linked_worktree: idx != 0,
                });
            app.state.workspaces[idx].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
                key: "repo-key".into(),
                repo_identity: "repo-key".into(),
                checkout_key: checkout_path.to_string(),
                repo_name: "herdr".into(),
                repo_root: "/repo/herdr".into(),
                is_linked_worktree: idx != 0,
            });
        }
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Navigate;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 30));
        let list = app.state.workspace_list_rect();
        assert!(!crate::ui::should_show_scrollbar(
            crate::ui::workspace_list_scroll_metrics(&app.state, list)
        ));

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, list.x + 1, list.y + 1));

        assert_eq!(app.state.selected, 2);
    }

    // Note (P2, bora-79l T1): this characterization runs in the default
    // Repo view, whose `Workspace` cards the card migration never touched
    // — the Project-view owner of this behavior (drag starting on the
    // PaneDotsRow block) is pinned in
    // `project_view_pane_dots_block_drag_opens_workspace_reorder_for_linked_worktree`
    // (src/app/input/mouse.rs).
    #[test]
    fn dragging_workspace_reorders_without_changing_identity() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            Workspace::test_new("a"),
            Workspace::test_new("b"),
            Workspace::test_new("c"),
        ];
        app.state.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        app.state.sidebar_spaces.row_gap = 0;
        for ws in &mut app.state.workspaces {
            ws.cached_git_branch = None;
        }
        let active_id = app.state.workspaces[1].id.clone();
        let selected_id = app.state.workspaces[2].id.clone();
        app.state.active = Some(1);
        app.state.selected = 2;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let packed_boundary_row = app.state.view.workspace_card_areas[1].rect.y;
        assert_eq!(
            app.state.workspace_drop_index_at_row(packed_boundary_row),
            Some(2)
        );

        let source_row = app.state.view.workspace_card_areas[1].rect.y;
        let target_row = crate::ui::workspace_drop_indicator_row(
            &app.state.view.workspace_card_areas,
            app.state.workspace_list_rect(),
            0,
        )
        .unwrap();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            source_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            target_row,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::WorkspaceReorder {
                source_ws_idx: 1,
                insert_idx: Some(0),
                ..
            })
        ));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, target_row));

        let names: Vec<_> = app
            .state
            .workspaces
            .iter()
            .map(crate::workspace::Workspace::display_name)
            .collect();
        assert_eq!(names, vec!["b", "a", "c"]);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.selected, 2);
        assert_eq!(app.state.workspaces[0].id, active_id);
        assert_eq!(app.state.workspaces[2].id, selected_id);
        let events = app.event_hub.events_after(0);
        assert!(events.iter().any(|(_, event)| matches!(
            event.data,
            crate::api::schema::EventData::WorkspaceMoved { .. }
        )));
        assert!(!events.iter().any(|(_, event)| matches!(
            event.data,
            crate::api::schema::EventData::WorkspaceReordered { .. }
        )));
        let snapshot = capture_snapshot(&app.state);
        let captured_names: Vec<_> = snapshot
            .workspaces
            .iter()
            .map(|ws| ws.custom_name.clone().unwrap())
            .collect();
        assert_eq!(captured_names, vec!["b", "a", "c"]);
    }

    #[test]
    fn flat_mode_drag_reorders_linked_worktree() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("main"), Workspace::test_new("issue")];
        for (idx, checkout_path) in ["/repo/herdr", "/repo/herdr-issue"].into_iter().enumerate() {
            app.state.workspaces[idx].worktree_space =
                Some(crate::workspace::WorktreeSpaceMembership {
                    key: "repo-key".into(),
                    label: "herdr".into(),
                    repo_root: "/repo/herdr".into(),
                    checkout_path: checkout_path.into(),
                    is_linked_worktree: idx > 0,
                });
            app.state.workspaces[idx].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
                key: "repo-key".into(),
                repo_identity: "repo-key".into(),
                checkout_key: checkout_path.to_string(),
                repo_name: "herdr".into(),
                repo_root: "/repo/herdr".into(),
                is_linked_worktree: idx > 0,
            });
        }
        app.state.active = None;
        app.state.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        app.state.sidebar_spaces.row_gap = 0;

        // --- Grouped mode (default): the linked worktree row refuses the drag. ---
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let issue_row = app
            .state
            .view
            .workspace_card_areas
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("issue card present")
            .rect
            .y;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, issue_row));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            issue_row + 5,
        ));
        assert!(app.state.drag.is_none());
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, issue_row));
        assert_eq!(
            app.state
                .workspaces
                .iter()
                .map(crate::workspace::Workspace::display_name)
                .collect::<Vec<_>>(),
            vec!["main", "issue"]
        );

        // --- Flat mode: the same linked worktree row is a free-standing,
        // directly drag-reorderable card. `active` tracks it by id, not by
        // index, so the drag must not lose the pointer.
        app.state.view_mode = crate::config::ViewMode::Flat;
        let active_id = app.state.workspaces[1].id.clone();
        app.state.active = Some(1);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let cards = app.state.view.workspace_card_areas.clone();
        let source_row = cards
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("issue card present")
            .rect
            .y;
        let target_row =
            crate::ui::workspace_drop_indicator_row(&cards, app.state.workspace_list_rect(), 0)
                .expect("drop row for insert_idx 0");

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            source_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            target_row,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::WorkspaceReorder {
                source_id: _,
                source_ws_idx: 1,
                insert_idx: Some(0),
            })
        ));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, target_row));

        assert_eq!(
            app.state
                .workspaces
                .iter()
                .map(crate::workspace::Workspace::display_name)
                .collect::<Vec<_>>(),
            vec!["issue", "main"]
        );
        // Identity survives the index shift: `active` still resolves to
        // "issue" by id, now at index 0.
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspaces[0].id, active_id);
    }

    #[test]
    fn folders_mode_drag_onto_group_header_assigns_visual_group() {
        let mut app = app_for_mouse_test();
        let mut ws0 = Workspace::test_new("alpha");
        ws0.visual_group = Some("g1".into());
        ws0.cached_git_branch = None;
        let mut ws1 = Workspace::test_new("loose");
        ws1.cached_git_branch = None;
        app.state.workspaces = vec![ws0, ws1];
        app.state.view_mode = crate::config::ViewMode::Folders;
        app.state.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        app.state.sidebar_spaces.row_gap = 0;
        app.state.active = None;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let header = app
            .state
            .view
            .workspace_group_header_areas
            .iter()
            .find(|h| h.name == "g1")
            .expect("g1 header rendered")
            .clone();
        let source_row = app
            .state
            .view
            .workspace_card_areas
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("loose card present")
            .rect
            .y;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            source_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            header.rect.y,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::WorkspaceReorder {
                source_ws_idx: 1,
                ..
            })
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            2,
            header.rect.y,
        ));

        assert_eq!(app.state.workspaces[1].visual_group.as_deref(), Some("g1"));
        assert!(app.state.drag.is_none());
    }

    #[test]
    fn folders_mode_drag_outside_header_still_reorders() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            Workspace::test_new("a"),
            Workspace::test_new("b"),
            Workspace::test_new("c"),
        ];
        app.state.view_mode = crate::config::ViewMode::Folders;
        app.state.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        app.state.sidebar_spaces.row_gap = 0;
        for ws in &mut app.state.workspaces {
            ws.cached_git_branch = None;
        }
        app.state.active = Some(1);
        app.state.selected = 2;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let source_row = app.state.view.workspace_card_areas[1].rect.y;
        let target_row = crate::ui::workspace_drop_indicator_row(
            &app.state.view.workspace_card_areas,
            app.state.workspace_list_rect(),
            0,
        )
        .unwrap();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            source_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            target_row,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::WorkspaceReorder {
                source_ws_idx: 1,
                insert_idx: Some(0),
                ..
            })
        ));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, target_row));

        let names: Vec<_> = app
            .state
            .workspaces
            .iter()
            .map(crate::workspace::Workspace::display_name)
            .collect();
        assert_eq!(names, vec!["b", "a", "c"]);
        assert!(app
            .state
            .workspaces
            .iter()
            .all(|ws| ws.visual_group.is_none()));
    }

    #[test]
    fn folders_mode_linked_worktree_drag_moves_one_row_not_the_repo_block() {
        // The fleet reality: nearly every row IS a linked worktree. Folders
        // drags must behave like Flat — the grabbed row travels ALONE — even
        // though `groups_workspaces()` is true there. Goes red if the Folders
        // exemption is dropped from `can_reorder` (drag never opens), from
        // `workspace_move_block_params`' linked-worktree guard (drop silently
        // swallowed), or from its `workspace_ids` expansion (the whole repo
        // sibling block travels with the grabbed row).
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Folders;
        let space = |linked: bool, checkout: &str| crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: checkout.into(),
            is_linked_worktree: linked,
        };
        let mut main = Workspace::test_new("main");
        main.worktree_space = Some(space(false, "/repo/herdr"));
        main.cached_git_branch = None;
        let mut wt_a = Workspace::test_new("wt-a");
        wt_a.worktree_space = Some(space(true, "/repo/herdr-a"));
        wt_a.cached_git_branch = None;
        let mut wt_b = Workspace::test_new("wt-b");
        wt_b.worktree_space = Some(space(true, "/repo/herdr-b"));
        wt_b.cached_git_branch = None;
        let mut loose = Workspace::test_new("loose");
        loose.cached_git_branch = None;
        app.state.workspaces = vec![main, wt_a, wt_b, loose];
        app.state.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        app.state.sidebar_spaces.row_gap = 0;
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let cards = app.state.view.workspace_card_areas.clone();
        let source_row = cards
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("wt-a card present")
            .rect
            .y;
        let area = app.state.workspace_list_rect();
        let bottom_row =
            crate::ui::workspace_drop_indicator_row(&cards, area, 4).expect("bottom slot present");

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            2,
            source_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            bottom_row,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::WorkspaceReorder {
                source_ws_idx: 1,
                insert_idx: Some(4),
                ..
            })
        ));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, bottom_row));

        let names: Vec<_> = app
            .state
            .workspaces
            .iter()
            .map(crate::workspace::Workspace::display_name)
            .collect();
        assert_eq!(names, vec!["main", "wt-b", "loose", "wt-a"]);
    }

    #[test]
    fn folders_mode_drop_slot_exists_between_same_repo_siblings() {
        // In Folders the list is flat and every row is an independent drag
        // root, so a slot must exist between two adjacent rows even when
        // they share a worktree key — the Repo-view gap suppression must
        // not leak in, or drops between same-repo siblings land on the
        // wrong side of the pair.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Folders;
        let mut workspaces = Vec::new();
        for (idx, name) in ["a", "b", "c"].into_iter().enumerate() {
            let mut ws = Workspace::test_new(name);
            ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
                key: "repo-key".into(),
                label: "herdr".into(),
                repo_root: "/repo/herdr".into(),
                checkout_path: format!("/repo/herdr-{idx}").into(),
                is_linked_worktree: true,
            });
            ws.cached_git_branch = None;
            workspaces.push(ws);
        }
        app.state.workspaces = workspaces;
        app.state.sidebar_spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        app.state.sidebar_spaces.row_gap = 0;
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let cards = app.state.view.workspace_card_areas.clone();
        let area = app.state.workspace_list_rect();
        let slot_before_b = crate::ui::workspace_drop_indicator_row(&cards, area, 1)
            .expect("slot before b present");

        assert_eq!(
            app.state.workspace_drop_index_at_row(slot_before_b),
            Some(1)
        );
    }

    #[test]
    fn clicking_tab_scroll_button_reveals_hidden_tabs_without_renaming() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(Some("logs"));
        ws.test_add_tab(Some("review"));
        ws.test_add_tab(Some("ops"));
        ws.test_add_tab(Some("notes"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 65, 20));

        let right = app.state.view.tab_scroll_right_hit_area;
        assert!(right.width > 0);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            right.x + 1,
            right.y,
        ));

        assert_eq!(app.state.tab_scroll, 1);
        assert!(!app.state.tab_scroll_follow_active);
        assert_eq!(app.state.workspaces[0].active_tab, 0);
        assert_eq!(app.state.view.tab_hit_areas[0].width, 0);
        assert!(app.state.workspaces[0].tabs[0].custom_name.is_none());
        assert_eq!(
            app.state.workspaces[0].tabs[1].custom_name.as_deref(),
            Some("logs")
        );
    }

    #[test]
    fn clicking_last_visible_tab_at_right_edge_does_not_overscroll() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        for name in [
            "one", "two", "three", "four", "five", "six", "seven", "eight",
        ] {
            ws.test_add_tab(Some(name));
        }
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.tab_scroll = usize::MAX;
        app.state.tab_scroll_follow_active = false;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 65, 20));

        let last_idx = app.state.workspaces[0].tabs.len() - 1;
        let target = app.state.view.tab_hit_areas[last_idx];
        let clamped_scroll = app.state.tab_scroll;
        assert!(target.width > 0, "last tab should already be visible");

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            target.x + 1,
            target.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            target.x + 1,
            target.y,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, last_idx);
        assert_eq!(app.state.tab_scroll, clamped_scroll);
        assert!(app.state.view.tab_hit_areas[last_idx].width > 0);
    }

    #[test]
    fn dragging_tab_reorders_auto_and_custom_names_without_materializing_numbers() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(Some("foo"));
        ws.test_add_tab(None);
        let moved_root = ws.tabs[0].root_pane;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let source = app.state.view.tab_hit_areas[0];
        let last = app.state.view.tab_hit_areas[2];
        let drop_col = last.x + last.width;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            source.x + 1,
            source.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            drop_col,
            source.y,
        ));
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::TabReorder {
                ws_idx: 0,
                source_tab_idx: 0,
                insert_idx: Some(3),
                ..
            })
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            drop_col,
            source.y,
        ));

        let labels: Vec<_> = app.state.workspaces[0]
            .tabs
            .iter()
            .enumerate()
            .map(|(tab_idx, _)| app.state.workspaces[0].tab_display_name(tab_idx).unwrap())
            .collect();
        assert_eq!(labels, vec!["foo", "2", "3"]);
        assert_eq!(
            app.state.workspaces[0].tabs[0].custom_name.as_deref(),
            Some("foo")
        );
        assert!(app.state.workspaces[0].tabs[1].custom_name.is_none());
        assert!(app.state.workspaces[0].tabs[2].custom_name.is_none());
        assert_eq!(app.state.workspaces[0].tabs[0].number, 2);
        assert_eq!(app.state.workspaces[0].tabs[1].number, 3);
        assert_eq!(app.state.workspaces[0].tabs[2].number, 1);
        assert_eq!(app.state.workspaces[0].tabs[2].root_pane, moved_root);
        assert_eq!(app.state.workspaces[0].active_tab, 2);
    }

    fn temp_git_repo(branch: &str) -> std::path::PathBuf {
        let repo = unique_temp_path("sidebar-drop-slot-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(
            repo.join(".git/HEAD"),
            format!("ref: refs/heads/{branch}\n"),
        )
        .unwrap();
        repo
    }

    fn workspace_with_space(name: &str, key: &str) -> Workspace {
        let mut ws = Workspace::test_new(name);
        ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: key.into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: format!("/repo/{name}").into(),
            is_linked_worktree: name != "main",
        });
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: key.into(),
            repo_identity: key.into(),
            checkout_key: format!("/repo/{name}"),
            repo_name: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            is_linked_worktree: name != "main",
        });
        ws
    }

    #[test]
    fn top_drop_slot_maps_to_first_workspace() {
        let mut app = app_for_mouse_test();
        let first_repo = temp_git_repo("main");
        let second_repo = temp_git_repo("main");

        let mut first = Workspace::test_new("a");
        let first_root = first.tabs[0].root_pane;
        first.identity_cwd = first_repo.clone();
        first.refresh_git_ahead_behind();

        let mut second = Workspace::test_new("b");
        let second_root = second.tabs[0].root_pane;
        second.identity_cwd = second_repo.clone();
        second.refresh_git_ahead_behind();

        app.state.workspaces = vec![first, second];
        app.state.ensure_test_terminals();
        let first_terminal_id = app.state.workspaces[0].tabs[0].panes[&first_root]
            .attached_terminal_id
            .clone();
        app.state.terminals.get_mut(&first_terminal_id).unwrap().cwd = first_repo.clone();
        let second_terminal_id = app.state.workspaces[1].tabs[0].panes[&second_root]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&second_terminal_id)
            .unwrap()
            .cwd = second_repo.clone();
        app.state.sidebar_spaces.row_gap = 1;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        assert_eq!(app.state.workspace_drop_index_at_row(0), Some(0));
        assert_eq!(app.state.workspace_drop_index_at_row(1), Some(0));
        assert_eq!(app.state.workspace_drop_index_at_row(2), Some(1));
        assert_eq!(app.state.workspace_drop_index_at_row(3), Some(1));

        let _ = fs::remove_dir_all(first_repo);
        let _ = fs::remove_dir_all(second_repo);
    }
    /// bora-uqv: right-click in Project view reaches the assembly menus — a
    /// project header (with Rename), the Ungrouped bucket (without Rename),
    /// and a workspace row (assembly section instead of the visual-group
    /// items, which must stay a flat/repo-only surface).
    #[test]
    fn project_view_right_click_reaches_header_and_row_menus() {
        let _isolated = crate::config::IsolatedDirs::new("project-view-right-click");
        let mut app = app_for_mouse_test();
        let repo_a = temp_git_repo("main");
        let repo_b = temp_git_repo("main");

        let mut a = Workspace::test_new("a");
        a.identity_cwd = repo_a.clone();
        a.refresh_git_ahead_behind();
        let mut b = Workspace::test_new("b");
        b.identity_cwd = repo_b.clone();
        b.refresh_git_ahead_behind();
        app.state.workspaces = vec![a, b];
        app.state.ensure_test_terminals();

        // alpha claims repo_a; repo_b stays an orphan.
        let dir_a = repo_a.display().to_string();
        crate::persist::projects::update_projects_file::<String>(move |file| {
            file.projects.insert(
                "alpha".to_string(),
                crate::persist::projects::Project {
                    name: None,
                    channel: None,
                    members: vec![crate::persist::projects::Member {
                        dir: dir_a,
                        worktrees: crate::persist::projects::WorktreesScope::All,
                        template: None,
                    }],
                    orchestrator: None,
                    sections: None,
                    layout: None,
                    auto_join: true,
                },
            );
            Ok(())
        })
        .unwrap();
        app.state.projects = crate::persist::projects::ProjectsStore::load();
        app.state.view_mode = crate::config::ViewMode::Project;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let project_row_rect = |app: &crate::app::App, key: &str| {
            app.state
                .view
                .project_row_areas
                .iter()
                .find(|area| {
                    matches!(
                        &area.target,
                        crate::app::state::ProjectRowTarget::Project { collapse_key }
                            if collapse_key == key
                    )
                })
                .unwrap_or_else(|| panic!("project header row for {key}"))
                .rect
        };
        let reset = |app: &mut crate::app::App| {
            app.state.context_menu = None;
            app.state.mode = Mode::Terminal;
        };

        // 1. A declared project header offers the full assembly menu.
        let rect = project_row_rect(&app, "proj:alpha");
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            rect.x + 1,
            rect.y,
        ));
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("project header menu");
        assert!(
            matches!(&menu.kind, crate::app::state::ContextMenuKind::ProjectHeader { slug, .. } if slug.as_deref() == Some("alpha")),
            "kind: {:?}",
            menu.kind
        );
        assert!(menu
            .items
            .iter()
            .any(|item| item == "Add workspaces\u{2026}"));
        assert!(menu.items.iter().any(|item| item == "New project\u{2026}"));
        assert!(menu
            .items
            .iter()
            .any(|item| item == "Rename project\u{2026}"));

        // 2. The Ungrouped bucket offers creation and the picker, never Rename.
        reset(&mut app);
        let rect = project_row_rect(&app, crate::ui::ORPHANS_COLLAPSE_KEY);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            rect.x + 1,
            rect.y,
        ));
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("orphans header menu");
        assert!(
            matches!(
                &menu.kind,
                crate::app::state::ContextMenuKind::ProjectHeader { slug: None, .. }
            ),
            "kind: {:?}",
            menu.kind
        );
        assert!(menu
            .items
            .iter()
            .any(|item| item == "Add workspaces\u{2026}"));
        assert!(menu.items.iter().any(|item| item == "New project\u{2026}"));
        assert!(!menu
            .items
            .iter()
            .any(|item| item == "Rename project\u{2026}"));

        // 3. The orphan workspace's block splices "Add to alpha" — and the
        // visual-group items are gone in Project view…
        //
        // Attribution (two rounds): this assertion read
        // `ProjectMemberTargets` until the SectionRow-card fix made
        // `workspace_at_row` resolve the row and reach the full
        // `GitWorkspace` menu. P2 (bora-79l T1) then moved the card one
        // row down — onto the workspace's own `PaneDotsRow` block — so
        // the full-menu right-click now happens THERE; the branch line
        // above keeps the narrow member-only menu. The item assertions
        // are unchanged and still pass from the block: the kind widened,
        // the membership items stayed.
        reset(&mut app);
        let block = app
            .state
            .view
            .workspace_card_areas
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("orphan workspace's PaneDotsRow block card")
            .rect;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            block.x + 1,
            block.y,
        ));
        let menu = app.state.context_menu.as_ref().expect("row menu");
        assert!(
            matches!(
                &menu.kind,
                crate::app::state::ContextMenuKind::GitWorkspace { ws_idx: 1, .. }
            ),
            "kind: {:?}",
            menu.kind
        );
        // The workspace-scoped items the owner lost are back alongside the
        // membership ones.
        assert!(menu.items.iter().any(|item| item == "Rename"));
        assert!(menu.items.iter().any(|item| item == "Add to alpha"));
        assert!(menu.items.iter().any(|item| item == "New project\u{2026}"));
        assert!(!menu.items.iter().any(|item| item == "New group\u{2026}"));

        // …but stays available in Repo view.
        reset(&mut app);
        app.state.view_mode = crate::config::ViewMode::Repo;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let card = app
            .state
            .view
            .workspace_card_areas
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("orphan workspace card in repo view")
            .rect;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            card.x + 1,
            card.y,
        ));
        let menu = app.state.context_menu.as_ref().expect("repo row menu");
        assert!(menu.items.iter().any(|item| item == "New group\u{2026}"));

        let _ = fs::remove_dir_all(repo_a);
        let _ = fs::remove_dir_all(repo_b);
    }

    #[test]
    fn bottom_drop_slot_stays_below_last_workspace_not_footer() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            Workspace::test_new("a"),
            Workspace::test_new("b"),
            Workspace::test_new("c"),
        ];
        for ws in &mut app.state.workspaces {
            ws.cached_git_branch = None;
        }
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 30));

        let cards = &app.state.view.workspace_card_areas;
        let end_idx = cards.last().map(|card| card.ws_idx + 1).unwrap();
        let bottom_slot = crate::ui::workspace_drop_indicator_row(
            cards,
            app.state.workspace_list_rect(),
            end_idx,
        )
        .unwrap();

        let last = cards.last().unwrap().rect;
        assert_eq!(bottom_slot, last.y + last.height);
        assert!(bottom_slot < app.state.sidebar_footer_rect().y.saturating_sub(1));
    }

    #[test]
    fn grouped_sidebar_drop_slots_do_not_land_inside_compact_group() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            workspace_with_space("main", "repo-key"),
            Workspace::test_new("normal"),
            workspace_with_space("issue", "repo-key"),
        ];
        app.state.active = Some(1);
        app.state.selected = 1;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 40));

        let cards = &app.state.view.workspace_card_areas;
        let order = cards.iter().map(|card| card.ws_idx).collect::<Vec<_>>();
        assert_eq!(order, vec![0, 2, 1]);
        let issue = cards.iter().find(|card| card.ws_idx == 2).unwrap();
        let normal = cards.iter().find(|card| card.ws_idx == 1).unwrap();

        assert_eq!(app.state.workspace_drop_index_at_row(issue.rect.y), Some(1));
        let end_idx = cards.last().map(|card| card.ws_idx + 1).unwrap();
        assert_eq!(
            crate::ui::workspace_drop_indicator_row(
                cards,
                app.state.workspace_list_rect(),
                end_idx,
            ),
            Some(normal.rect.y + normal.rect.height)
        );
    }

    #[test]
    fn dragging_worktree_space_member_does_not_reorder_workspaces() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            workspace_with_space("main", "repo-key"),
            Workspace::test_new("normal"),
            workspace_with_space("issue", "repo-key"),
        ];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 40));

        let source = app
            .state
            .view
            .workspace_card_areas
            .iter()
            .find(|card| card.ws_idx == 2)
            .unwrap()
            .rect;
        let target_row = crate::ui::workspace_drop_indicator_row(
            &app.state.view.workspace_card_areas,
            app.state.workspace_list_rect(),
            0,
        )
        .unwrap();

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, source.y));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            2,
            target_row,
        ));
        assert!(app.state.drag.is_none());
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, target_row));

        let names = app
            .state
            .workspaces
            .iter()
            .map(crate::workspace::Workspace::display_name)
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["main", "normal", "issue"]);
    }

    #[test]
    fn dragging_sidebar_divider_sets_manual_width() {
        let mut app = app_for_mouse_test();

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 30, 5));

        assert_eq!(app.state.sidebar_width, 31);
        let snapshot = capture_snapshot(&app.state);
        assert_eq!(snapshot.sidebar_width, Some(31));
    }

    #[test]
    fn dragging_sidebar_bottom_divider_still_sets_manual_width() {
        let mut app = app_for_mouse_test();
        let divider_col = app.state.view.sidebar_rect.x + app.state.view.sidebar_rect.width - 1;
        let bottom_row = app.state.view.sidebar_rect.y + app.state.view.sidebar_rect.height - 1;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            divider_col,
            bottom_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            divider_col + 5,
            bottom_row,
        ));

        assert_eq!(app.state.sidebar_width, 31);
    }

    #[test]
    fn dragging_past_max_clamps_to_configured_max() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_max_width = 30;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 50, 5));

        assert_eq!(app.state.sidebar_width, 30);
    }

    #[test]
    fn dragging_below_min_clamps_to_configured_min() {
        let mut app = app_for_mouse_test();
        app.state.sidebar_min_width = 22;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 5));

        assert_eq!(app.state.sidebar_width, 22);
    }

    /// The section divider is gone with the agent panel (bora-49p.6), so there
    /// is nothing to drag. `sidebar_section_split` itself was deliberately NOT
    /// deleted, so old sessions still restore — that round-trip is the part
    /// still worth guarding, and it was previously only covered as a side
    /// effect of the drag this test used to perform.
    #[test]
    fn retired_section_divider_cannot_be_dragged_but_the_split_still_persists() {
        let mut app = app_for_mouse_test();
        let divider = crate::ui::sidebar_section_divider_rect(
            app.state.view.sidebar_rect,
            app.state.sidebar_section_split,
        );
        assert_eq!(divider, ratatui::layout::Rect::default());

        let before = app.state.sidebar_section_split;
        let sidebar = app.state.view.sidebar_rect;
        // The sweep stops above the footer row: the "new" launcher would
        // create a workspace mid-drag — footer territory, not the retired
        // divider's (see the retired-panel test for the same boundary).
        for row in sidebar.y..sidebar.y + sidebar.height - 1 {
            app.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                sidebar.x + 1,
                row,
            ));
            app.handle_mouse(mouse(
                MouseEventKind::Drag(MouseButton::Left),
                sidebar.x + 1,
                row + 4,
            ));
        }
        assert_eq!(app.state.sidebar_section_split, before);

        // Still persisted, so a session written by a build that had the panel
        // reads back unchanged.
        app.state.sidebar_section_split = 0.37;
        let snapshot = capture_snapshot(&app.state);
        assert_eq!(snapshot.sidebar_section_split, Some(0.37));
    }

    #[test]
    fn double_clicking_sidebar_divider_resets_default_width() {
        let mut app = app_for_mouse_test();
        app.state.default_sidebar_width = 26;
        app.state.sidebar_width = 30;

        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 25, 5));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 25, 5));

        assert_eq!(app.state.sidebar_width, 26);
        assert!(app.state.drag.is_none());
        let snapshot = capture_snapshot(&app.state);
        assert_eq!(snapshot.sidebar_width, Some(26));
    }
}
