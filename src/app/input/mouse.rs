use bytes::Bytes;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Direction, Rect};
use tracing::warn;

use crate::{
    app::state::{
        build_context_menu_items, AgentPanelSort, AppState, ContextMenuKind, ContextMenuState,
        DragState, DragTarget, MenuListState, Mode, ProjectRowTarget, RightClickPassthroughGesture,
        TabPressState, ViewLayout, WorkspacePressState,
    },
    layout::{PaneInfo, SplitBorder},
    selection::Selection,
    terminal::TerminalRuntimeRegistry,
};

#[cfg(test)]
use super::WheelRouting;
use super::{
    modal::{
        apply_global_menu_action, confirm_close_cancel, global_menu_actions, leave_modal,
        modal_action_from_buttons, open_global_menu, open_new_tab_dialog, ModalAction,
    },
    settings::SettingsAction,
    sidebar::project_row_target_at,
    ScrollbarClickTarget, TAB_DRAG_THRESHOLD, WORKSPACE_DRAG_THRESHOLD,
};

pub(super) enum MouseAction {
    NewWorkspace,
    Settings(SettingsAction),
    FocusWorkspace {
        ws_idx: usize,
    },
    FocusTab {
        tab_idx: usize,
    },
    FocusPane {
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    },
    FocusToastTarget,
    MoveWorkspace {
        source_ws_idx: usize,
        insert_idx: usize,
    },
    MoveWorkspaceBlock {
        params: crate::api::schema::WorkspaceMoveBlockParams,
    },
    MoveTab {
        ws_idx: usize,
        source_tab_idx: usize,
        insert_idx: usize,
    },
    SetSplitRatio {
        path: Vec<bool>,
        ratio: f32,
    },
    RenameModal(ModalAction),
    ConfirmCloseAccept,
    ContextMenu {
        menu: ContextMenuState,
        idx: usize,
    },
}

enum MobileMouseResult {
    Ignored,
    Consumed,
    Action(MouseAction),
}

impl AppState {
    pub(crate) fn handle_pane_mouse_only(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        mouse: MouseEvent,
    ) {
        if self.mode != Mode::Terminal {
            return;
        }
        let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() else {
            return;
        };

        match mouse.kind {
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => {
                self.forward_pane_reported_wheel(terminal_runtimes, &info, mouse);
            }
            MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => {
                self.forward_pane_mouse_button(terminal_runtimes, &info, mouse);
            }
            MouseEventKind::Moved => {
                self.forward_pane_mouse_motion(terminal_runtimes, &info, mouse);
            }
        }
    }

    pub(super) fn handle_mouse(
        &mut self,
        terminal_runtimes: &mut TerminalRuntimeRegistry,
        source_id: crate::app::InputSourceId,
        mouse: MouseEvent,
    ) -> Option<MouseAction> {
        if self.mode == Mode::Onboarding {
            self.handle_onboarding_mouse(mouse);
            return None;
        }

        if self.mode == Mode::Terminal
            && self.clickable_toast_at(mouse.column, mouse.row)
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            return Some(MouseAction::FocusToastTarget);
        }

        if self.mode == Mode::Terminal
            && self.clickable_toast_at(mouse.column, mouse.row)
            && matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left))
        {
            return None;
        }

        if self.mode == Mode::Settings {
            return self.handle_settings_mouse(mouse).map(MouseAction::Settings);
        }

        let launcher_enabled = self.view.layout != ViewLayout::Mobile
            && !self.sidebar_collapsed
            && matches!(
                self.mode,
                Mode::Terminal
                    | Mode::Navigate
                    | Mode::Resize
                    | Mode::GlobalMenu
                    | Mode::KeybindHelp
            );
        let launcher = self.global_launcher_rect();
        let launcher_hit = launcher_enabled
            && mouse.column >= launcher.x
            && mouse.column < launcher.x + launcher.width
            && mouse.row >= launcher.y
            && mouse.row < launcher.y + launcher.height;

        if matches!(mouse.kind, MouseEventKind::Moved) && self.mode == Mode::GlobalMenu {
            let actions = global_menu_actions(self);
            let hovered = self
                .global_menu_item_at(mouse.column, mouse.row)
                .and_then(|action| actions.iter().position(|item| *item == action));
            self.global_menu.hover(hovered);
            return None;
        }

        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) && launcher_hit {
            if self.mode == Mode::GlobalMenu {
                leave_modal(self);
            } else {
                open_global_menu(self);
            }
            return None;
        }

        if self.mode == Mode::GlobalMenu {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                if let Some(action) = self.global_menu_item_at(mouse.column, mouse.row) {
                    apply_global_menu_action(self, action);
                } else {
                    leave_modal(self);
                }
            }
            return None;
        }

        if self.mode == Mode::KeybindHelp {
            return None;
        }

        if self.view.layout == ViewLayout::Mobile {
            match self.handle_mobile_mouse(mouse) {
                MobileMouseResult::Ignored => {}
                MobileMouseResult::Consumed => return None,
                MobileMouseResult::Action(action) => return Some(action),
            }
        }

        let sidebar = self.view.sidebar_rect;
        let in_sidebar = mouse.column >= sidebar.x
            && mouse.column < sidebar.x + sidebar.width
            && mouse.row >= sidebar.y
            && mouse.row < sidebar.y + sidebar.height;
        let right_panel = self.view.right_panel_rect;
        let in_right_panel = right_panel.width > 0
            && mouse.column >= right_panel.x
            && mouse.column < right_panel.x + right_panel.width
            && mouse.row >= right_panel.y
            && mouse.row < right_panel.y + right_panel.height;

        if self.handle_right_click_passthrough(
            terminal_runtimes,
            source_id,
            mouse,
            in_sidebar || in_right_panel,
        ) {
            return None;
        }

        if self.mode == Mode::OpenExistingWorktree {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(open) = &mut self.worktree_open {
                        open.select_previous_filtered();
                    }
                    return None;
                }
                MouseEventKind::ScrollDown => {
                    if let Some(open) = &mut self.worktree_open {
                        open.select_next_filtered();
                    }
                    return None;
                }
                _ => {}
            }
        }

        if matches!(
            self.mode,
            Mode::NewLinkedWorktree | Mode::OpenExistingWorktree | Mode::ConfirmRemoveWorktree
        ) && !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            return None;
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.selection = None;
                self.selection_autoscroll = None;
                self.clear_chrome_press(source_id);

                if self.mode == Mode::ConfirmClose {
                    let popup = self.confirm_close_rect();
                    let inner = Rect::new(
                        popup.x + 1,
                        popup.y + 1,
                        popup.width.saturating_sub(2),
                        popup.height.saturating_sub(2),
                    );
                    let (confirm, cancel) = crate::ui::confirm_close_button_rects(inner);
                    match modal_action_from_buttons(
                        mouse.column,
                        mouse.row,
                        &[
                            (confirm, ModalAction::Confirm),
                            (cancel, ModalAction::Cancel),
                        ],
                    ) {
                        Some(ModalAction::Confirm) => {
                            return Some(MouseAction::ConfirmCloseAccept);
                        }
                        Some(ModalAction::Cancel) | None => confirm_close_cancel(self),
                        _ => {}
                    }
                    return None;
                }

                if self.mode == Mode::NewLinkedWorktree {
                    if let Some(inner) =
                        crate::ui::new_linked_worktree_inner_rect(self.screen_rect())
                    {
                        use crate::app::state::WorktreeCreateTab;
                        // Tab-strip click switches the active tab.
                        let tab_rects = crate::ui::create_worktree_tab_rects(inner);
                        for (rect, tab) in tab_rects.iter().zip([
                            WorktreeCreateTab::Github,
                            WorktreeCreateTab::Branch,
                            WorktreeCreateTab::Name,
                        ]) {
                            if mouse.row == rect.y
                                && mouse.column >= rect.x
                                && mouse.column < rect.x + rect.width
                            {
                                if let Some(create) = self.worktree_create.as_mut() {
                                    create.active_tab = tab;
                                }
                                return None;
                            }
                        }
                        // List-row click selects the row on the list tabs.
                        if let Some(tab) = self.worktree_create.as_ref().map(|c| c.active_tab) {
                            let entries_len = match tab {
                                WorktreeCreateTab::Github => {
                                    self.create_worktree_github_entries().len()
                                }
                                WorktreeCreateTab::Branch => {
                                    self.create_worktree_branch_entries().len()
                                }
                                WorktreeCreateTab::Name => 0,
                            };
                            if entries_len > 0 {
                                let selected = self
                                    .worktree_create
                                    .as_ref()
                                    .map(|c| match tab {
                                        WorktreeCreateTab::Github => c.github_pick.selected,
                                        WorktreeCreateTab::Branch => c.branch_pick.selected,
                                        WorktreeCreateTab::Name => 0,
                                    })
                                    .unwrap_or(0)
                                    .min(entries_len - 1);
                                let max_rows = crate::ui::create_worktree_list_visible_rows(inner);
                                let start = crate::ui::create_worktree_list_start(
                                    selected,
                                    entries_len,
                                    max_rows,
                                );
                                let visible = max_rows.min(entries_len - start);
                                for visible_idx in 0..visible {
                                    let row = crate::ui::create_worktree_list_row_rect(
                                        inner,
                                        visible_idx,
                                    );
                                    if mouse.row == row.y
                                        && mouse.column >= row.x
                                        && mouse.column < row.x + row.width
                                    {
                                        let idx = start + visible_idx;
                                        if let Some(create) = self.worktree_create.as_mut() {
                                            match tab {
                                                WorktreeCreateTab::Github => {
                                                    create.github_pick.selected = idx
                                                }
                                                WorktreeCreateTab::Branch => {
                                                    create.branch_pick.selected = idx
                                                }
                                                WorktreeCreateTab::Name => {}
                                            }
                                        }
                                        return None;
                                    }
                                }
                            }
                        }
                        let (create, cancel) = crate::ui::new_linked_worktree_button_rects(inner);
                        match modal_action_from_buttons(
                            mouse.column,
                            mouse.row,
                            &[
                                (create, ModalAction::Confirm),
                                (cancel, ModalAction::Cancel),
                            ],
                        ) {
                            Some(ModalAction::Confirm) => {
                                self.request_submit_worktree_create = true;
                            }
                            Some(ModalAction::Cancel)
                                if !self
                                    .worktree_create
                                    .as_ref()
                                    .is_some_and(|create| create.creating) =>
                            {
                                self.worktree_create = None;
                                self.name_input.clear();
                                self.name_input_replace_on_type = false;
                                leave_modal(self);
                            }
                            _ => {}
                        }
                    }
                    return None;
                }

                if self.mode == Mode::OpenExistingWorktree {
                    if let Some(open) = self.worktree_open.as_ref() {
                        if let Some(inner) = crate::ui::open_existing_worktree_inner_rect(
                            self.screen_rect(),
                            open.entries.len(),
                        ) {
                            let filtered = open.filtered_indices();
                            let max_rows =
                                crate::ui::open_existing_worktree_max_visible_rows(inner);
                            let start =
                                crate::ui::open_existing_worktree_visible_start(open, max_rows);
                            if mouse.row == inner.y.saturating_add(1)
                                && mouse.column >= inner.x
                                && mouse.column < inner.x.saturating_add(inner.width)
                            {
                                if let Some(open) = &mut self.worktree_open {
                                    open.search_focused = true;
                                }
                                return None;
                            }
                            let row_idx = if rect_contains(inner, mouse.column, mouse.row) {
                                mouse
                                    .row
                                    .checked_sub(inner.y.saturating_add(3))
                                    .map(usize::from)
                                    .map(|row| row / 2)
                                    .filter(|row| *row < max_rows)
                                    .and_then(|row| filtered.get(start + row).copied())
                            } else {
                                None
                            };
                            if let Some(entry_idx) = row_idx {
                                if let Some(open) = &mut self.worktree_open {
                                    open.selected = entry_idx;
                                }
                                self.request_submit_worktree_open = true;
                                return None;
                            }

                            let (open_button, cancel) =
                                crate::ui::open_existing_worktree_button_rects(inner);
                            match modal_action_from_buttons(
                                mouse.column,
                                mouse.row,
                                &[
                                    (open_button, ModalAction::Confirm),
                                    (cancel, ModalAction::Cancel),
                                ],
                            ) {
                                Some(ModalAction::Confirm) => {
                                    self.request_submit_worktree_open = true;
                                }
                                Some(ModalAction::Cancel) => {
                                    self.worktree_open = None;
                                    leave_modal(self);
                                }
                                _ => {}
                            }
                        }
                    }
                    return None;
                }

                if self.mode == Mode::ConfirmRemoveWorktree {
                    if let Some(popup) = crate::ui::remove_worktree_popup_rect(self.screen_rect()) {
                        let inner = Rect::new(
                            popup.x + 1,
                            popup.y + 1,
                            popup.width.saturating_sub(2),
                            popup.height.saturating_sub(2),
                        );
                        let force_confirmation = self
                            .worktree_remove
                            .as_ref()
                            .is_some_and(|remove| remove.force_confirmation);
                        let mergeable = self
                            .worktree_remove
                            .as_ref()
                            .is_some_and(|remove| remove.branch.is_some());
                        let removing = self
                            .worktree_remove
                            .as_ref()
                            .is_some_and(|remove| remove.removing);
                        let (merge_rect, remove, cancel) = crate::ui::remove_worktree_button_rects(
                            inner,
                            force_confirmation,
                            mergeable,
                        );
                        if let Some(merge_rect) = merge_rect {
                            if !removing
                                && mouse.column >= merge_rect.x
                                && mouse.column < merge_rect.x.saturating_add(merge_rect.width)
                                && mouse.row >= merge_rect.y
                                && mouse.row < merge_rect.y.saturating_add(merge_rect.height)
                            {
                                self.request_submit_worktree_merge = true;
                                return None;
                            }
                        }
                        match modal_action_from_buttons(
                            mouse.column,
                            mouse.row,
                            &[
                                (remove, ModalAction::Confirm),
                                (cancel, ModalAction::Cancel),
                            ],
                        ) {
                            Some(ModalAction::Confirm) => {
                                self.request_submit_worktree_remove = true;
                            }
                            Some(ModalAction::Cancel)
                                if !self
                                    .worktree_remove
                                    .as_ref()
                                    .is_some_and(|remove| remove.removing) =>
                            {
                                self.worktree_remove = None;
                                leave_modal(self);
                            }
                            _ => {}
                        }
                    }
                    return None;
                }

                if matches!(
                    self.mode,
                    Mode::RenameWorkspace
                        | Mode::RenameTab
                        | Mode::RenamePane
                        | Mode::SetWorkspaceGroup
                        | Mode::ProjectNameInput
                ) {
                    let action = self
                        .rename_modal_inner()
                        .map(crate::ui::rename_button_rects)
                        .and_then(|(save, clear, cancel)| {
                            modal_action_from_buttons(
                                mouse.column,
                                mouse.row,
                                &[
                                    (save, ModalAction::Save),
                                    (clear, ModalAction::Clear),
                                    (cancel, ModalAction::Cancel),
                                ],
                            )
                        })
                        .unwrap_or(ModalAction::Cancel);
                    return Some(MouseAction::RenameModal(action));
                }

                if self.mode == Mode::ContextMenu {
                    let item_idx = self.context_menu_item_at(mouse.column, mouse.row);
                    if let Some(menu) = self.context_menu.take() {
                        if let Some(idx) = item_idx {
                            return Some(MouseAction::ContextMenu { menu, idx });
                        } else {
                            leave_modal(self);
                        }
                    }
                    return None;
                }

                if self.on_sidebar_divider(mouse.column, mouse.row) {
                    self.drag = Some(DragState {
                        target: DragTarget::SidebarDivider,
                    });
                    self.set_manual_sidebar_width(mouse.column);
                    return None;
                }

                if self.on_sidebar_section_divider(mouse.column, mouse.row) {
                    self.drag = Some(DragState {
                        target: DragTarget::SidebarSectionDivider,
                    });
                    self.set_sidebar_section_split(mouse.row);
                    return None;
                }

                if self.on_right_panel_divider(mouse.column, mouse.row) {
                    // ponytail: divider hit consumed; resize drag deferred until config supports it
                    return None;
                }

                // Right panel toggle — works when expanded OR collapsed
                if self.on_right_panel_toggle(mouse.column, mouse.row) {
                    self.right_panel_collapsed = !self.right_panel_collapsed;
                    return None;
                }

                if in_right_panel {
                    let rp = self.view.right_panel_rect;
                    // Content starts 1 col after the left separator
                    let content_y = rp.y;
                    if mouse.row == content_y {
                        // Tab header click — segment hit-test shared with the renderer
                        if let Some(tab) =
                            crate::ui::right_panel::right_panel_tab_hit(mouse.column, rp)
                        {
                            if tab != self.right_panel_active_tab {
                                self.right_panel_active_tab = tab;
                                self.right_panel_scroll = 0;
                                match tab {
                                    crate::app::state::RightPanelTab::Changes => {}
                                    crate::app::state::RightPanelTab::Checks => {
                                        self.right_panel_checks_requested = true;
                                    }
                                    crate::app::state::RightPanelTab::Issues => {
                                        self.right_panel_issues_requested = true;
                                    }
                                    crate::app::state::RightPanelTab::PullRequests => {
                                        self.right_panel_prs_requested = true;
                                    }
                                }
                            }
                        }
                    } else if mouse.row > content_y
                        && self.right_panel_active_tab == crate::app::state::RightPanelTab::Changes
                    {
                        // File row click — resolve which file was clicked
                        if let Some(file_path) = self.right_panel_file_at_row(mouse.row) {
                            self.right_panel_selected_file = Some(file_path);
                            self.right_panel_diff_requested = true;
                        }
                    } else if mouse.row > content_y
                        && self.right_panel_active_tab == crate::app::state::RightPanelTab::Issues
                    {
                        // Issue row click — open the issue context menu
                        if let Some((number, url)) = self.right_panel_issue_at_row(mouse.row) {
                            let flow_available = self.repo_issue_flow_template().is_some();
                            let kind = ContextMenuKind::RepoIssue {
                                number,
                                url,
                                flow_available,
                            };
                            self.context_menu = Some(ContextMenuState {
                                items: build_context_menu_items(
                                    &kind,
                                    &self.workspaces,
                                    self.view_mode,
                                    &[],
                                    &[],
                                    &self.installed_plugins,
                                ),
                                kind,
                                x: mouse.column,
                                y: mouse.row,
                                list: MenuListState::new(0),
                                bora_commands: vec![],
                                bora_port: None,
                            });
                            self.mode = Mode::ContextMenu;
                        }
                    } else if mouse.row > content_y
                        && self.right_panel_active_tab
                            == crate::app::state::RightPanelTab::PullRequests
                    {
                        // PR row click — open the PR context menu
                        if let Some((number, url, head_ref)) = self.right_panel_pr_at_row(mouse.row)
                        {
                            let ws_idx = self.active.unwrap_or(0);
                            let kind = ContextMenuKind::RepoPr {
                                ws_idx,
                                number,
                                url,
                                head_ref,
                            };
                            self.context_menu = Some(ContextMenuState {
                                items: build_context_menu_items(
                                    &kind,
                                    &self.workspaces,
                                    self.view_mode,
                                    &[],
                                    &[],
                                    &self.installed_plugins,
                                ),
                                kind,
                                x: mouse.column,
                                y: mouse.row,
                                list: MenuListState::new(0),
                                bora_commands: vec![],
                                bora_port: None,
                            });
                            self.mode = Mode::ContextMenu;
                        }
                    }
                    return None;
                }

                if !in_sidebar && !in_right_panel {
                    if let Some(border) = self.find_border_at(mouse.column, mouse.row) {
                        let grab_offset = match border.direction {
                            Direction::Horizontal => border.pos.saturating_sub(mouse.column),
                            Direction::Vertical => border.pos.saturating_sub(mouse.row),
                        };
                        self.drag = Some(DragState {
                            target: DragTarget::PaneSplit {
                                path: border.path.clone(),
                                direction: border.direction,
                                area: border.area,
                                grab_offset,
                            },
                        });
                        return None;
                    }

                    if let Some((pane_id, target)) =
                        self.scrollbar_target_at(terminal_runtimes, mouse.column, mouse.row)
                    {
                        self.focus_pane(pane_id);
                        match target {
                            ScrollbarClickTarget::Thumb { grab_row_offset } => {
                                self.drag = Some(DragState {
                                    target: DragTarget::PaneScrollbar {
                                        pane_id,
                                        grab_row_offset,
                                    },
                                });
                            }
                            ScrollbarClickTarget::Track { offset_from_bottom } => {
                                self.set_pane_scroll_offset(
                                    terminal_runtimes,
                                    pane_id,
                                    offset_from_bottom,
                                );
                            }
                        }
                        if self.mode != Mode::Terminal {
                            self.mode = Mode::Terminal;
                        }
                        return None;
                    }
                }

                if self.mode_bar_covers_tab_row(mouse.column, mouse.row) {
                    return None;
                }

                if self.on_tab_scroll_left_button(mouse.column, mouse.row) {
                    self.scroll_tabs_left();
                    return None;
                }
                if self.on_tab_scroll_right_button(mouse.column, mouse.row) {
                    self.scroll_tabs_right();
                    return None;
                }
                if let (Some(ws_idx), Some(tab_idx)) =
                    (self.active, self.tab_at(mouse.column, mouse.row))
                {
                    self.tab_presses.insert(
                        source_id,
                        TabPressState {
                            ws_idx,
                            tab_idx,
                            start_col: mouse.column,
                            start_row: mouse.row,
                        },
                    );
                    return None;
                }
                if self.on_new_tab_button(mouse.column, mouse.row) {
                    if self.prompt_new_tab_name {
                        open_new_tab_dialog(self);
                    } else {
                        self.request_new_tab = true;
                        self.mode = Mode::Terminal;
                    }
                    return None;
                }

                if in_sidebar {
                    if self.on_sidebar_toggle(mouse.column, mouse.row) {
                        self.sidebar_collapsed = !self.sidebar_collapsed;
                        self.request_full_repaint();
                        return None;
                    }

                    if self.sidebar_collapsed {
                        if let Some(idx) = self.collapsed_workspace_at_row(mouse.row) {
                            self.mode = Mode::Terminal;
                            return Some(MouseAction::FocusWorkspace { ws_idx: idx });
                        }

                        if let Some((ws_idx, _tab_idx, pane_id)) =
                            self.collapsed_agent_detail_target_at(mouse.row)
                        {
                            self.mode = Mode::Terminal;
                            return Some(MouseAction::FocusPane { ws_idx, pane_id });
                        }
                        return None;
                    }

                    let new_button = self.sidebar_new_button_rect();
                    let on_new_button = mouse.row >= new_button.y
                        && mouse.row < new_button.y + new_button.height
                        && mouse.column >= new_button.x
                        && mouse.column < new_button.x + new_button.width;
                    if on_new_button {
                        return Some(MouseAction::NewWorkspace);
                    }

                    if self.on_view_mode_toggle(mouse.column, mouse.row) {
                        self.view_mode = self.view_mode.cycle();
                        self.workspace_scroll = 0;
                        self.mark_session_dirty();
                        self.request_full_repaint();
                        return None;
                    }

                    if !self.view.project_row_areas.is_empty() {
                        if let Some(target) = project_row_target_at(
                            &self.view.project_row_areas,
                            mouse.column,
                            mouse.row,
                        )
                        .cloned()
                        {
                            // P2, bora-79l T1: the branch line is NOT a
                            // workspace row anymore — the workspace's own
                            // `PaneDotsRow` block carries its card and its
                            // press (via `workspace_at_row` below). A
                            // `Section` target only keeps its caret
                            // column's collapse; everywhere else on the
                            // row records no press, so neither a click
                            // (mouse-up's `chrome_press_action`) nor a
                            // Drag can switch workspace from the branch
                            // line. Every other `ProjectRowTarget`
                            // (Project, Band, SectionItem, Pane, OpenPr,
                            // OpenWorktree) stays exactly as click-only
                            // as before.
                            if let ProjectRowTarget::Section { .. } = &target {
                                // Split the row by column (owner's ask: "eu
                                // clico no nome da workspace e ele
                                // retrai" — clicking anywhere used to
                                // collapse). T3 (bora-79l) removed the
                                // `▾`/`▸` chevron from the branch header
                                // (collapse belongs to the folder), but
                                // the geometry pass still gives this
                                // row's `ProjectRowHitArea.rect` the same
                                // `x` the render used, so the row's
                                // first TWO cells — formerly the
                                // chevron's column, now the leading
                                // `⌗`/`⎇` slot — remain the collapse
                                // target, read back here rather than a
                                // hardcoded literal. The visible
                                // affordance for that column is T6's to
                                // design.
                                let on_caret = self
                                    .view
                                    .project_row_areas
                                    .iter()
                                    .find(|area| {
                                        mouse.column >= area.rect.x
                                            && mouse.column < area.rect.x + area.rect.width
                                            && mouse.row >= area.rect.y
                                            && mouse.row < area.rect.y + area.rect.height
                                    })
                                    // The caret glyph plus its separating
                                    // space, i.e. the row's first TWO cells.
                                    // One cell is a hostile mouse target in
                                    // a mouse-first TUI, and the second cell
                                    // is the chevron's own trailing space —
                                    // it belongs to the caret visually, so
                                    // spending it here costs no reachable
                                    // part of the name.
                                    .is_some_and(|area| mouse.column < area.rect.x + 2);
                                if !on_caret {
                                    // P2, bora-79l T1: the branch line no
                                    // longer selects the workspace — no
                                    // `WorkspacePressState` is recorded, so
                                    // neither mouse-up's
                                    // `chrome_press_action` nor a Drag past
                                    // the threshold can act on this row.
                                    // The workspace's own affordances now
                                    // live one row down: its `PaneDotsRow`
                                    // block's `WorkspaceCardArea` feeds
                                    // `workspace_presses` via
                                    // `workspace_at_row` below.
                                    return None;
                                }
                            }
                            return self.handle_project_row_click(target);
                        }
                    }

                    if let Some(target) =
                        self.workspace_list_scrollbar_target_at(mouse.column, mouse.row)
                    {
                        match target {
                            ScrollbarClickTarget::Thumb { grab_row_offset } => {
                                self.drag = Some(DragState {
                                    target: DragTarget::WorkspaceListScrollbar { grab_row_offset },
                                });
                            }
                            ScrollbarClickTarget::Track { offset_from_bottom } => {
                                self.set_workspace_list_offset_from_bottom(offset_from_bottom);
                            }
                        }
                        return None;
                    }

                    let cards = if self.view.workspace_card_areas.is_empty() {
                        crate::ui::compute_workspace_card_areas(self, self.view.sidebar_rect)
                    } else {
                        self.view.workspace_card_areas.clone()
                    };
                    // The "+" affordance on a repo header row opens the Create
                    // worktree modal; checked before the header collapse hit so
                    // it wins over toggling the row's collapse state.
                    for hit in &self.view.worktree_new_hit_areas.clone() {
                        if mouse.row == hit.rect.y
                            && mouse.column >= hit.rect.x
                            && mouse.column < hit.rect.x + hit.rect.width
                        {
                            self.request_open_create_worktree = Some(hit.repo_identity.clone());
                            return None;
                        }
                    }
                    // Check for clicks on visual group headers.
                    for header in &self.view.workspace_group_header_areas.clone() {
                        if mouse.row == header.rect.y
                            && mouse.column >= header.rect.x
                            && mouse.column < header.rect.x + header.rect.width
                        {
                            let key = header.collapse_key.clone();
                            if key == "hidden:" {
                                self.hidden_section_expanded = !self.hidden_section_expanded;
                                return None;
                            }
                            if self.collapsed_space_keys.contains(&key) {
                                self.collapsed_space_keys.remove(&key);
                            } else {
                                self.collapsed_space_keys.insert(key);
                            }
                            self.mark_session_dirty();
                            return None;
                        }
                    }

                    if let Some(card) = cards.iter().find(|card| {
                        let chevron = crate::ui::workspace_group_chevron_rect(card);
                        mouse.row == chevron.y && mouse.column == chevron.x && chevron.width > 0
                    }) {
                        if let Some((key, collapsed)) =
                            crate::ui::workspace_parent_group_state(self, card.ws_idx)
                        {
                            if collapsed {
                                self.collapsed_space_keys.remove(&key);
                            } else {
                                self.collapsed_space_keys.insert(key);
                            }
                            self.mark_session_dirty();
                            return None;
                        }
                    }

                    if let Some(idx) = self.workspace_at_row(mouse.row) {
                        self.workspace_presses.insert(
                            source_id,
                            WorkspacePressState {
                                ws_idx: idx,
                                start_col: mouse.column,
                                start_row: mouse.row,
                            },
                        );
                        return None;
                    }

                    if self.on_agent_panel_sort_toggle(mouse.column, mouse.row) {
                        self.agent_panel_sort = match self.agent_panel_sort {
                            AgentPanelSort::Spaces => AgentPanelSort::Priority,
                            AgentPanelSort::Priority => AgentPanelSort::Spaces,
                        };
                        self.agent_panel_scroll = 0;
                        self.mark_session_dirty();
                        return None;
                    }

                    if let Some(target) =
                        self.agent_panel_scrollbar_target_at(mouse.column, mouse.row)
                    {
                        match target {
                            ScrollbarClickTarget::Thumb { grab_row_offset } => {
                                self.drag = Some(DragState {
                                    target: DragTarget::AgentPanelScrollbar { grab_row_offset },
                                });
                            }
                            ScrollbarClickTarget::Track { offset_from_bottom } => {
                                self.set_agent_panel_offset_from_bottom(offset_from_bottom);
                            }
                        }
                        return None;
                    }

                    if let Some((ws_idx, _tab_idx, pane_id)) =
                        self.agent_detail_target_at(mouse.row)
                    {
                        self.mode = Mode::Terminal;
                        return Some(MouseAction::FocusPane { ws_idx, pane_id });
                    }
                } else if let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() {
                    if self.mode != Mode::Terminal {
                        self.mode = Mode::Terminal;
                    }

                    if self.forward_pane_mouse_button(terminal_runtimes, &info, mouse) {
                        self.selection = None;
                        self.selection_autoscroll = None;
                        return self.mouse_pane_focus_action(info.id);
                    }

                    let (row, col) = (
                        mouse.row - info.inner_rect.y,
                        mouse.column - info.inner_rect.x,
                    );
                    self.selection = Some(Selection::anchor(
                        info.id,
                        row,
                        col,
                        self.pane_scroll_metrics(terminal_runtimes, info.id),
                    ));
                    return self.mouse_pane_focus_action(info.id);
                } else if let Some(info) = self.view.pane_infos.iter().find(|p| {
                    mouse.column >= p.rect.x
                        && mouse.column < p.rect.x + p.rect.width
                        && mouse.row >= p.rect.y
                        && mouse.row < p.rect.y + p.rect.height
                }) {
                    let id = info.id;
                    if self.mode != Mode::Terminal {
                        self.mode = Mode::Terminal;
                    }
                    return self.mouse_pane_focus_action(id);
                }
            }

            MouseEventKind::Drag(MouseButton::Left) => {
                if self.selection.is_some() {
                    self.update_selection_drag(terminal_runtimes, mouse.column, mouse.row);
                    return None;
                }

                if (self.drag.is_none() || self.chrome_drag_owned_by_other(source_id))
                    && !self.chrome_press_pending(source_id)
                {
                    if let Some(info) = self.pane_mouse_target(mouse.column, mouse.row).cloned() {
                        if self.forward_pane_mouse_button(terminal_runtimes, &info, mouse) {
                            self.selection = None;
                            self.selection_autoscroll = None;
                            return None;
                        }
                    }
                }

                let workspace_drop_index = self.workspace_drop_index_at_row(mouse.row);
                let tab_drop_index = self.tab_drop_index_at(mouse.column, mouse.row);
                if self.drag.is_none() {
                    if let Some(press) = self.workspace_presses.get(&source_id) {
                        let delta_col = mouse.column.abs_diff(press.start_col);
                        let delta_row = mouse.row.abs_diff(press.start_row);
                        let can_reorder = self.workspaces.get(press.ws_idx).is_some_and(|ws| {
                            // In Repo view a linked worktree renders nested
                            // under its main checkout's group card and has no
                            // independent position of its own — only the
                            // group root can be dragged, which is why this
                            // guard exists at all. Project view is different
                            // by construction (bora-c1h): every workspace,
                            // linked worktree or not, gets its own top-level
                            // `SectionRow`/`WorkspaceCardArea`, and
                            // `workspace_move_block_params`'s own `roots`
                            // computation already treats every `SectionRow`
                            // as a drag root (see its `WorkspaceListEntry`
                            // match). So the "linked worktrees can't reorder
                            // independently" restriction must not apply
                            // there, or virtually every Project-view row
                            // (nearly all of them ARE linked worktrees) would
                            // silently refuse to drag — the exact regression
                            // reported.
                            !self.groups_workspaces()
                                || self.view_mode == crate::config::ViewMode::Project
                                || ws
                                    .worktree_space()
                                    .is_none_or(|space| !space.is_linked_worktree)
                        });
                        if workspace_drop_index.is_some()
                            && can_reorder
                            && delta_col.max(delta_row) >= WORKSPACE_DRAG_THRESHOLD
                        {
                            self.drag = Some(DragState {
                                target: DragTarget::WorkspaceReorder {
                                    source_id,
                                    source_ws_idx: press.ws_idx,
                                    insert_idx: workspace_drop_index,
                                },
                            });
                        }
                    } else if let Some(press) = self.tab_presses.get(&source_id) {
                        let delta_col = mouse.column.abs_diff(press.start_col);
                        let delta_row = mouse.row.abs_diff(press.start_row);
                        // Require a real drop target before opening a reorder,
                        // so a report from off the tab bar cannot start a drag
                        // that has nowhere to land.
                        if tab_drop_index.is_some()
                            && delta_col.max(delta_row) >= TAB_DRAG_THRESHOLD
                        {
                            self.drag = Some(DragState {
                                target: DragTarget::TabReorder {
                                    source_id,
                                    ws_idx: press.ws_idx,
                                    source_tab_idx: press.tab_idx,
                                    insert_idx: tab_drop_index,
                                },
                            });
                        }
                    }
                }

                if let Some(DragState {
                    target:
                        DragTarget::WorkspaceReorder {
                            source_id: drag_source_id,
                            insert_idx,
                            ..
                        },
                }) = &mut self.drag
                {
                    if *drag_source_id == source_id {
                        *insert_idx = workspace_drop_index;
                    }
                } else if let Some(DragState {
                    target:
                        DragTarget::TabReorder {
                            source_id: drag_source_id,
                            ws_idx,
                            insert_idx,
                            ..
                        },
                }) = &mut self.drag
                {
                    if *drag_source_id == source_id && self.active == Some(*ws_idx) {
                        *insert_idx = tab_drop_index;
                    }
                } else if let Some(drag) = &self.drag {
                    match &drag.target {
                        DragTarget::WorkspaceReorder { .. } | DragTarget::TabReorder { .. } => {}
                        DragTarget::WorkspaceListScrollbar { grab_row_offset } => {
                            if let Some(offset_from_bottom) =
                                self.workspace_list_offset_for_drag_row(mouse.row, *grab_row_offset)
                            {
                                self.set_workspace_list_offset_from_bottom(offset_from_bottom);
                            }
                        }
                        DragTarget::AgentPanelScrollbar { grab_row_offset } => {
                            if let Some(offset_from_bottom) =
                                self.agent_panel_offset_for_drag_row(mouse.row, *grab_row_offset)
                            {
                                self.set_agent_panel_offset_from_bottom(offset_from_bottom);
                            }
                        }
                        DragTarget::PaneSplit {
                            path,
                            direction,
                            area,
                            grab_offset,
                        } => {
                            let ratio = match direction {
                                Direction::Horizontal => {
                                    f32::from(
                                        mouse
                                            .column
                                            .saturating_add(*grab_offset)
                                            .saturating_sub(area.x),
                                    ) / f32::from(area.width.max(1))
                                }
                                Direction::Vertical => {
                                    f32::from(
                                        mouse
                                            .row
                                            .saturating_add(*grab_offset)
                                            .saturating_sub(area.y),
                                    ) / f32::from(area.height.max(1))
                                }
                            };
                            let ratio = ratio.clamp(0.1, 0.9);
                            let path = path.clone();
                            return Some(MouseAction::SetSplitRatio { path, ratio });
                        }
                        DragTarget::PaneScrollbar {
                            pane_id,
                            grab_row_offset,
                        } => {
                            if let Some(offset_from_bottom) = self.scrollbar_offset_for_pane_row(
                                terminal_runtimes,
                                *pane_id,
                                mouse.row,
                                *grab_row_offset,
                            ) {
                                self.set_pane_scroll_offset(
                                    terminal_runtimes,
                                    *pane_id,
                                    offset_from_bottom,
                                );
                            }
                        }
                        DragTarget::SidebarDivider => {
                            self.set_manual_sidebar_width(mouse.column);
                        }
                        DragTarget::SidebarSectionDivider => {
                            self.set_sidebar_section_split(mouse.row);
                        }
                        DragTarget::ReleaseNotesScrollbar { .. }
                        | DragTarget::ProductAnnouncementScrollbar { .. }
                        | DragTarget::KeybindHelpScrollbar { .. } => {}
                    }
                }
            }

            MouseEventKind::Up(MouseButton::Left) => {
                // Mouse-up either finishes a drag selection or releases after a
                // double-click word selection; the latter is already finalized.
                if let Some(selection) = self.selection.as_ref() {
                    let was_click = selection.was_just_click();
                    let was_finalized = selection.is_finalized();

                    self.clear_chrome_press(source_id);
                    self.drag = None;
                    self.selection_autoscroll = None;
                    if was_click {
                        self.selection = None;
                    } else if was_finalized {
                        // Double-click already finalized this word selection.
                    } else if self.copy_on_select {
                        self.copy_selection(terminal_runtimes);
                    } else if let Some(selection) = self.selection.as_mut() {
                        selection.finish();
                    }
                    return None;
                }

                let foreign_chrome_drag = self.chrome_drag_owned_by_other(source_id);
                if (self.drag.is_none() || foreign_chrome_drag)
                    && !self.chrome_press_pending(source_id)
                {
                    if let Some(info) = self.pane_mouse_target(mouse.column, mouse.row).cloned() {
                        if self.forward_pane_mouse_button(terminal_runtimes, &info, mouse) {
                            self.selection = None;
                            self.selection_autoscroll = None;
                            return None;
                        }
                    }
                }

                let workspace_press = self.workspace_presses.remove(&source_id);
                let tab_press = self.tab_presses.remove(&source_id);
                if foreign_chrome_drag {
                    return self.chrome_press_action(workspace_press, tab_press);
                }

                match self.drag.take() {
                    Some(DragState {
                        target:
                            DragTarget::WorkspaceReorder {
                                source_ws_idx,
                                insert_idx: Some(insert_idx),
                                ..
                            },
                    }) => {
                        // A drop over a Project-view PROJECT HEADER row
                        // (not another workspace row) re-parents the
                        // dragged workspace into that project instead of
                        // reordering it — "navigate between different
                        // groups" by drag. Checked first so it wins
                        // cleanly over the reorder path below; dropping on
                        // a `Section`/other row target leaves `reparent`
                        // `None` and falls through unchanged.
                        // `ProjectRowTarget::Project`'s `collapse_key` is
                        // `proj:{slug}` for a declared project (bora-uqv,
                        // see `push_project_group`) or `ORPHANS_COLLAPSE_KEY`
                        // for the implicit `declared: false` orphan bucket,
                        // which carries no slug — dropping there clears the
                        // binding, the exact inverse of an explicit
                        // assignment, and what `set_project(None)` means.
                        let reparent = project_row_target_at(
                            &self.view.project_row_areas,
                            mouse.column,
                            mouse.row,
                        )
                        .and_then(|target| match target {
                            ProjectRowTarget::Project { collapse_key }
                                if collapse_key == crate::ui::ORPHANS_COLLAPSE_KEY =>
                            {
                                Some(None)
                            }
                            ProjectRowTarget::Project { collapse_key } => {
                                Some(collapse_key.strip_prefix("proj:").map(str::to_string))
                            }
                            _ => None,
                        });
                        if let Some(slug) = reparent {
                            if let Some(ws) = self.workspaces.get_mut(source_ws_idx) {
                                ws.set_project(slug);
                            }
                            self.mark_session_dirty();
                            self.request_full_repaint();
                            return None;
                        }
                        if !self.groups_workspaces() {
                            // Flat mode: every row is an independent drag
                            // target, never a block. `insert_idx` is already
                            // a raw vec position; `move_workspace` no-ops on
                            // an out-of-range or unchanged target.
                            return Some(MouseAction::MoveWorkspace {
                                source_ws_idx,
                                insert_idx,
                            });
                        }
                        if let Some(params) =
                            self.workspace_move_block_params(source_ws_idx, insert_idx)
                        {
                            if self
                                .workspaces
                                .get(source_ws_idx)
                                .is_some_and(|workspace| workspace.worktree_space().is_some())
                            {
                                return Some(MouseAction::MoveWorkspaceBlock { params });
                            }
                            let insert_idx = params
                                .before_workspace_id
                                .as_ref()
                                .and_then(|id| {
                                    self.workspaces
                                        .iter()
                                        .position(|workspace| workspace.id == *id)
                                })
                                .unwrap_or(self.workspaces.len());
                            return Some(MouseAction::MoveWorkspace {
                                source_ws_idx,
                                insert_idx,
                            });
                        }
                    }
                    Some(DragState {
                        target:
                            DragTarget::TabReorder {
                                ws_idx,
                                source_tab_idx,
                                insert_idx: Some(insert_idx),
                                ..
                            },
                    }) => {
                        if self.active == Some(ws_idx) {
                            self.mode = Mode::Terminal;
                            return Some(MouseAction::MoveTab {
                                ws_idx,
                                source_tab_idx,
                                insert_idx,
                            });
                        }
                    }
                    Some(_) => {}
                    None => return self.chrome_press_action(workspace_press, tab_press),
                }
            }

            MouseEventKind::Up(MouseButton::Middle) | MouseEventKind::Drag(MouseButton::Middle)
                if !in_sidebar && !in_right_panel =>
            {
                if let Some(info) = self.pane_mouse_target(mouse.column, mouse.row).cloned() {
                    let _ = self.forward_pane_mouse_button(terminal_runtimes, &info, mouse);
                }
            }

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if self.mode_bar_covers_tab_row(mouse.column, mouse.row) => {}

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if self.on_tab_bar(mouse.column, mouse.row) =>
            {
                match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        if let Some(ws) = self.active.and_then(|i| self.workspaces.get(i)) {
                            if !ws.tabs.is_empty() {
                                let prev = if ws.active_tab == 0 {
                                    ws.tabs.len() - 1
                                } else {
                                    ws.active_tab - 1
                                };
                                return Some(MouseAction::FocusTab { tab_idx: prev });
                            }
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        if let Some(ws) = self.active.and_then(|i| self.workspaces.get(i)) {
                            if !ws.tabs.is_empty() {
                                let next = (ws.active_tab + 1) % ws.tabs.len();
                                return Some(MouseAction::FocusTab { tab_idx: next });
                            }
                        }
                    }
                    _ => {}
                }
            }

            MouseEventKind::ScrollUp if in_right_panel => {
                self.right_panel_scroll = self.right_panel_scroll.saturating_sub(1);
            }
            MouseEventKind::ScrollDown if in_right_panel => {
                self.right_panel_scroll = self.right_panel_scroll.saturating_add(1);
            }

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if !in_sidebar
                    && !in_right_panel
                    && self.scroll_selection_with_wheel(terminal_runtimes, mouse) => {}

            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if !in_sidebar && !in_right_panel =>
            {
                self.selection = None;
                self.selection_autoscroll = None;
                self.handle_terminal_wheel(terminal_runtimes, mouse);
            }

            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                if self.mode == Mode::Terminal && !in_sidebar =>
            {
                if let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() {
                    self.forward_pane_reported_wheel(terminal_runtimes, &info, mouse);
                }
            }

            MouseEventKind::ScrollUp if in_sidebar => {
                let agent_area = self.agent_panel_rect();
                let over_agent_panel = agent_area != Rect::default()
                    && mouse.row >= agent_area.y
                    && mouse.row < agent_area.y + agent_area.height;
                if over_agent_panel {
                    if crate::ui::should_show_scrollbar(crate::ui::agent_panel_scroll_metrics(
                        self, agent_area,
                    )) {
                        self.scroll_agent_panel(-1);
                    }
                } else if crate::ui::should_show_scrollbar(
                    crate::ui::workspace_list_scroll_metrics(self, self.workspace_list_rect()),
                ) {
                    self.scroll_workspace_list(-1);
                } else {
                    self.move_selected_workspace_by_visible_delta(-1);
                }
            }
            MouseEventKind::ScrollDown if in_sidebar => {
                let agent_area = self.agent_panel_rect();
                let over_agent_panel = agent_area != Rect::default()
                    && mouse.row >= agent_area.y
                    && mouse.row < agent_area.y + agent_area.height;
                if over_agent_panel {
                    if crate::ui::should_show_scrollbar(crate::ui::agent_panel_scroll_metrics(
                        self, agent_area,
                    )) {
                        self.scroll_agent_panel(1);
                    }
                } else if crate::ui::should_show_scrollbar(
                    crate::ui::workspace_list_scroll_metrics(self, self.workspace_list_rect()),
                ) {
                    self.scroll_workspace_list(1);
                } else {
                    self.move_selected_workspace_by_visible_delta(1);
                }
            }

            MouseEventKind::Moved if self.mode == Mode::ContextMenu => {
                let hovered = self.context_menu_item_at(mouse.column, mouse.row);
                if let Some(menu) = &mut self.context_menu {
                    menu.list.hover(hovered);
                }
            }

            MouseEventKind::Moved
                if self.mode == Mode::Terminal && !in_sidebar && !in_right_panel =>
            {
                if let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() {
                    let _ = self.forward_pane_mouse_motion(terminal_runtimes, &info, mouse);
                }
            }

            MouseEventKind::Down(MouseButton::Right) if in_sidebar && !self.sidebar_collapsed => {
                self.clear_chrome_press(source_id);
                if self
                    .workspace_list_scrollbar_target_at(mouse.column, mouse.row)
                    .is_some()
                {
                    return None;
                }
                if let Some(idx) = self.workspace_at_row(mouse.row) {
                    self.selected = idx;
                    let hidden = self
                        .workspaces
                        .get(idx)
                        .is_some_and(|ws| self.is_hidden(&format!("ws:{}", ws.id)));
                    let kind = self
                        .workspaces
                        .get(idx)
                        .and_then(|ws| {
                            let group_state = crate::ui::workspace_parent_group_state(self, idx);
                            let git_space = ws.git_space().cloned().or_else(|| {
                                ws.resolved_identity_cwd_from(&self.terminals, terminal_runtimes)
                                    .as_deref()
                                    .and_then(crate::workspace::git_space_metadata)
                            });
                            let is_linked_worktree = ws.worktree_space().map_or_else(
                                || {
                                    git_space
                                        .as_ref()
                                        .is_some_and(|space| space.is_linked_worktree)
                                },
                                |space| space.is_linked_worktree,
                            );
                            let show_git_menu = ws.worktree_space().is_some()
                                || git_space
                                    .as_ref()
                                    .is_some_and(|space| !space.is_linked_worktree);
                            show_git_menu.then_some(ContextMenuKind::GitWorkspace {
                                ws_idx: idx,
                                is_linked_worktree,
                                has_worktree_children: group_state.is_some(),
                                collapsed: group_state
                                    .as_ref()
                                    .is_some_and(|(_, collapsed)| *collapsed),
                                hidden,
                            })
                        })
                        .unwrap_or(ContextMenuKind::Workspace {
                            ws_idx: idx,
                            hidden,
                        });
                    // Load .bora.toml commands for workspace context menus.
                    let (bora_labels, bora_commands, bora_port) = if matches!(
                        kind,
                        ContextMenuKind::Workspace { .. } | ContextMenuKind::GitWorkspace { .. }
                    ) {
                        let ws = &self.workspaces[idx];
                        let filtered = crate::bora_config::workspace_commands(ws);
                        if filtered.is_empty() {
                            (vec![], vec![], None)
                        } else {
                            let labels: Vec<String> =
                                filtered.iter().map(|c| c.label.clone()).collect();
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
                            let port = ws.bora_config_root().and_then(|root| {
                                crate::bora_settings::resolve_port(root, checkout_path, &key)
                            });
                            (labels, filtered, port)
                        }
                    } else {
                        (vec![], vec![], None)
                    };
                    let mut items = build_context_menu_items(
                        &kind,
                        &self.workspaces,
                        self.view_mode,
                        &{
                            // bora-uqv: in Project view the row menu
                            // splices project membership items where the
                            // visual-group items would be.
                            if self.view_mode == crate::config::ViewMode::Project {
                                super::modal::workspace_assembly_items(&self.workspaces, idx)
                            } else {
                                Vec::new()
                            }
                        },
                        &bora_labels,
                        &self.installed_plugins,
                    );
                    // bora-79l.10 T6b: the workspace's own PaneDotsRow block
                    // is a Project-view section's row too — splice the
                    // section controls on top of everything the block's
                    // menu already offers, never in place of it. Bead
                    // bora-79l.7 (F5) was supposed to land this and did not.
                    if self.view_mode == crate::config::ViewMode::Project {
                        if let Some(checkout_key) = self
                            .workspaces
                            .get(idx)
                            .map(crate::workspace::Workspace::project_member_dir)
                        {
                            items.extend(super::modal::section_menu_items_for_checkout(
                                &self.projects,
                                &checkout_key,
                            ));
                        }
                    }
                    self.context_menu = Some(ContextMenuState {
                        items,
                        kind,
                        x: mouse.column,
                        y: mouse.row,
                        list: MenuListState::new(0),
                        bora_commands,
                        bora_port,
                    });
                    self.mode = Mode::ContextMenu;
                } else if let Some(header) = self
                    .view
                    .workspace_group_header_areas
                    .iter()
                    .find(|h| {
                        mouse.row == h.rect.y
                            && mouse.column >= h.rect.x
                            && mouse.column < h.rect.x + h.rect.width
                    })
                    .filter(|h| h.collapse_key != "hidden:")
                    .cloned()
                {
                    let hidden = self.is_hidden(&header.collapse_key);
                    let kind = ContextMenuKind::GroupHeader {
                        name: header.name.clone(),
                        collapse_key: header.collapse_key,
                        hidden,
                    };
                    self.context_menu = Some(ContextMenuState {
                        items: build_context_menu_items(
                            &kind,
                            &self.workspaces,
                            self.view_mode,
                            &[],
                            &[],
                            &self.installed_plugins,
                        ),
                        kind,
                        x: mouse.column,
                        y: mouse.row,
                        list: MenuListState::new(0),
                        bora_commands: vec![],
                        bora_port: None,
                    });
                    self.mode = Mode::ContextMenu;
                } else if self.view_mode == crate::config::ViewMode::Project {
                    // bora-uqv: right-click on a Project-view project header
                    // (or the Ungrouped bucket) opens the assembly menu; on a
                    // worktree/checkout row, the membership menu for that dir.
                    match project_row_target_at(
                        &self.view.project_row_areas,
                        mouse.column,
                        mouse.row,
                    )
                    .cloned()
                    {
                        Some(ProjectRowTarget::Project { collapse_key }) => {
                            let slug = if collapse_key == crate::ui::ORPHANS_COLLAPSE_KEY {
                                None
                            } else {
                                collapse_key.strip_prefix("proj:").map(str::to_string)
                            };
                            let hidden = self.is_hidden(&collapse_key);
                            let mut items = Vec::new();
                            if !super::modal::orphan_member_dirs(self).is_empty() {
                                items.push("Add workspaces\u{2026}".to_string());
                            }
                            items.push("New project\u{2026}".to_string());
                            if slug.is_some() {
                                items.push("Rename project\u{2026}".to_string());
                            }
                            items.push(crate::app::state::CONTEXT_MENU_SEPARATOR.to_string());
                            if hidden {
                                items.push("Unhide".to_string());
                            } else {
                                items.push("Hide 5m".to_string());
                                items.push("Hide 10m".to_string());
                                items.push("Hide 15m".to_string());
                                items.push("Hide 30m".to_string());
                            }
                            self.context_menu = Some(ContextMenuState {
                                items,
                                kind: ContextMenuKind::ProjectHeader {
                                    slug,
                                    collapse_key,
                                    hidden,
                                },
                                x: mouse.column,
                                y: mouse.row,
                                list: MenuListState::new(0),
                                bora_commands: vec![],
                                bora_port: None,
                            });
                            self.mode = Mode::ContextMenu;
                        }
                        Some(ProjectRowTarget::Section { checkout_key, .. }) => {
                            let mut items = super::modal::assembly_items_for_dir(&checkout_key);
                            // bora-79l.10 T6b: bead bora-79l.7 (F5) was
                            // supposed to land the section controls on this
                            // row and did not — splice them on top of the
                            // membership items, never in place of them.
                            items.extend(super::modal::section_menu_items_for_checkout(
                                &self.projects,
                                &checkout_key,
                            ));
                            self.context_menu = Some(ContextMenuState {
                                items,
                                kind: ContextMenuKind::ProjectMemberTargets {
                                    member_dir: checkout_key,
                                },
                                x: mouse.column,
                                y: mouse.row,
                                list: MenuListState::new(0),
                                bora_commands: vec![],
                                bora_port: None,
                            });
                            self.mode = Mode::ContextMenu;
                        }
                        _ => {}
                    }
                }
            }

            MouseEventKind::Down(MouseButton::Right)
                if !self.mode_bar_covers_tab_row(mouse.column, mouse.row)
                    && self.tab_at(mouse.column, mouse.row).is_some() =>
            {
                if let (Some(ws_idx), Some(tab_idx)) =
                    (self.active, self.tab_at(mouse.column, mouse.row))
                {
                    let kind = ContextMenuKind::Tab { ws_idx, tab_idx };
                    self.context_menu = Some(ContextMenuState {
                        items: build_context_menu_items(
                            &kind,
                            &self.workspaces,
                            self.view_mode,
                            &[],
                            &[],
                            &self.installed_plugins,
                        ),
                        kind,
                        x: mouse.column,
                        y: mouse.row,
                        list: MenuListState::new(0),
                        bora_commands: vec![],
                        bora_port: None,
                    });
                    self.mode = Mode::ContextMenu;
                }
            }

            MouseEventKind::Down(MouseButton::Right) if !in_sidebar && !in_right_panel => {
                if let Some(info) = self.pane_mouse_target(mouse.column, mouse.row).cloned() {
                    let ws_idx = self.active?;
                    let tab_idx = self
                        .workspaces
                        .get(ws_idx)
                        .map(crate::workspace::Workspace::active_tab_index)?;
                    let previous_focused_pane_id = self
                        .workspaces
                        .get(ws_idx)
                        .and_then(crate::workspace::Workspace::focused_pane_id);
                    let source_pane_id =
                        previous_focused_pane_id.filter(|pane_id| *pane_id != info.id);
                    let pane_state = self
                        .workspaces
                        .get(ws_idx)
                        .and_then(|ws| ws.pane_state(info.id));
                    let has_manual_label = pane_state
                        .and_then(|pane| self.terminals.get(&pane.attached_terminal_id))
                        .and_then(|terminal| terminal.manual_label.as_ref())
                        .is_some();
                    let right_click_passthrough =
                        pane_state.is_some_and(|pane| pane.right_click_passthrough);
                    let kind = ContextMenuKind::Pane {
                        ws_idx,
                        tab_idx,
                        pane_id: info.id,
                        source_pane_id,
                        has_manual_label,
                        right_click_passthrough,
                    };
                    self.context_menu = Some(ContextMenuState {
                        items: build_context_menu_items(
                            &kind,
                            &self.workspaces,
                            self.view_mode,
                            &[],
                            &[],
                            &self.installed_plugins,
                        ),
                        kind,
                        x: mouse.column,
                        y: mouse.row,
                        list: MenuListState::new(0),
                        bora_commands: vec![],
                        bora_port: None,
                    });
                    self.mode = Mode::ContextMenu;
                }
            }

            _ => {}
        }

        None
    }

    fn handle_mobile_mouse(&mut self, mouse: MouseEvent) -> MobileMouseResult {
        if self.mode == Mode::Navigate {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_mobile_switcher_at(mouse.column, mouse.row, -1);
                    return MobileMouseResult::Consumed;
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_mobile_switcher_at(mouse.column, mouse.row, 1);
                    return MobileMouseResult::Consumed;
                }
                MouseEventKind::Down(MouseButton::Left) => {}
                _ => return MobileMouseResult::Consumed,
            }
        } else if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return MobileMouseResult::Ignored;
        }

        if self.mode != Mode::Navigate {
            if !matches!(self.mode, Mode::Terminal | Mode::Resize) {
                return MobileMouseResult::Ignored;
            }
            if rect_contains(self.view.mobile_menu_hit_area, mouse.column, mouse.row) {
                self.mobile_switcher_scroll = 0;
                self.mode = Mode::Navigate;
                return MobileMouseResult::Consumed;
            }
            if rect_contains(self.view.mobile_prev_tab_hit_area, mouse.column, mouse.row) {
                self.previous_tab();
                return MobileMouseResult::Consumed;
            }
            if rect_contains(self.view.mobile_next_tab_hit_area, mouse.column, mouse.row) {
                self.next_tab();
                return MobileMouseResult::Consumed;
            }
            return MobileMouseResult::Ignored;
        }

        let areas = crate::ui::mobile_switcher_areas(self);
        if rect_contains(areas.close, mouse.column, mouse.row) {
            self.mode = Mode::Terminal;
            return MobileMouseResult::Consumed;
        }

        match crate::ui::mobile_switcher_target_at(self, mouse.column, mouse.row) {
            Some(crate::ui::MobileSwitcherTarget::NewWorkspace) => {
                return MobileMouseResult::Action(MouseAction::NewWorkspace);
            }
            Some(crate::ui::MobileSwitcherTarget::Workspace(ws_idx)) => {
                self.mode = Mode::Terminal;
                return MobileMouseResult::Action(MouseAction::FocusWorkspace { ws_idx });
            }
            Some(crate::ui::MobileSwitcherTarget::NewTab) => {
                if self.prompt_new_tab_name {
                    open_new_tab_dialog(self);
                } else {
                    self.request_new_tab = true;
                    self.mode = Mode::Terminal;
                }
            }
            Some(crate::ui::MobileSwitcherTarget::Tab(tab_idx)) => {
                self.mode = Mode::Terminal;
                return MobileMouseResult::Action(MouseAction::FocusTab { tab_idx });
            }
            Some(crate::ui::MobileSwitcherTarget::Agent {
                ws_idx,
                tab_idx: _,
                pane_id,
            }) => {
                self.mode = Mode::Terminal;
                return MobileMouseResult::Action(MouseAction::FocusPane { ws_idx, pane_id });
            }
            Some(crate::ui::MobileSwitcherTarget::Menu(action_idx)) => {
                let actions = global_menu_actions(self);
                if let Some(action) = actions.get(action_idx).copied() {
                    apply_global_menu_action(self, action);
                }
            }
            None => {}
        }

        MobileMouseResult::Consumed
    }

    fn scroll_mobile_switcher_at(&mut self, _col: u16, _row: u16, delta: i16) {
        let max_scroll = crate::ui::mobile_switcher_max_scroll(self);
        apply_scroll(
            &mut self.mobile_switcher_scroll,
            delta.saturating_mul(2),
            max_scroll,
        );
    }

    pub(super) fn screen_rect(&self) -> Rect {
        let sidebar = self.view.sidebar_rect;
        let terminal = self.view.terminal_area;
        let x = sidebar.x.min(terminal.x);
        let y = sidebar.y.min(terminal.y);
        let right = (sidebar.x + sidebar.width).max(terminal.x + terminal.width);
        let bottom = (sidebar.y + sidebar.height).max(terminal.y + terminal.height);
        Rect::new(x, y, right.saturating_sub(x), bottom.saturating_sub(y))
    }

    pub(crate) fn context_menu_rect(&self) -> Option<Rect> {
        let menu = self.context_menu.as_ref()?;
        let screen = self.screen_rect();
        let max_item_w = menu
            .items()
            .iter()
            .map(|item| item.len() as u16)
            .max()
            .unwrap_or(0);
        let menu_w = (max_item_w + 4).max(14).min(screen.width.max(1));
        let menu_h = (menu.items().len() as u16 + 2).min(screen.height.max(1));
        let x = menu.x.min(screen.x + screen.width.saturating_sub(menu_w));
        let y = menu.y.min(screen.y + screen.height.saturating_sub(menu_h));
        Some(Rect::new(x, y, menu_w, menu_h))
    }

    pub(crate) fn confirm_close_rect(&self) -> Rect {
        crate::ui::confirm_close_popup_rect(self.view.terminal_area).unwrap_or_default()
    }

    fn context_menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu_rect = self.context_menu_rect()?;
        let inner_x = menu_rect.x + 1;
        let inner_y = menu_rect.y + 1;
        let inner_w = menu_rect.width.saturating_sub(2);
        let inner_h = menu_rect.height.saturating_sub(2);
        let item_count = self
            .context_menu
            .as_ref()
            .map(|menu| menu.items().len() as u16)
            .unwrap_or(0);
        if col >= inner_x
            && col < inner_x + inner_w
            && row >= inner_y
            && row < inner_y + inner_h.min(item_count)
        {
            Some((row - inner_y) as usize)
        } else {
            None
        }
    }

    pub(super) fn tab_at(&self, col: u16, row: u16) -> Option<usize> {
        self.view
            .tab_hit_areas
            .iter()
            .enumerate()
            .find_map(|(idx, area)| {
                (area.width > 0
                    && row >= area.y
                    && row < area.y + area.height
                    && col >= area.x
                    && col < area.x + area.width)
                    .then_some(idx)
            })
    }

    fn mode_bar_covers_tab_row(&self, col: u16, row: u16) -> bool {
        self.tab_bar_position == crate::config::TabBarPositionConfig::Bottom
            && matches!(
                self.mode,
                Mode::Navigate | Mode::Prefix | Mode::Copy | Mode::Resize
            )
            && self.on_tab_bar(col, row)
    }

    pub(super) fn on_tab_bar(&self, col: u16, row: u16) -> bool {
        let area = self.view.tab_bar_rect;
        area.width > 0
            && row >= area.y
            && row < area.y + area.height
            && col >= area.x
            && col < area.x + area.width
    }

    pub(super) fn on_tab_scroll_left_button(&self, col: u16, row: u16) -> bool {
        let area = self.view.tab_scroll_left_hit_area;
        area.width > 0
            && row >= area.y
            && row < area.y + area.height
            && col >= area.x
            && col < area.x + area.width
    }

    pub(super) fn on_tab_scroll_right_button(&self, col: u16, row: u16) -> bool {
        let area = self.view.tab_scroll_right_hit_area;
        area.width > 0
            && row >= area.y
            && row < area.y + area.height
            && col >= area.x
            && col < area.x + area.width
    }

    pub(super) fn tab_drop_index_at(&self, col: u16, row: u16) -> Option<usize> {
        if !self.on_tab_bar(col, row) {
            return None;
        }

        let visible_tabs: Vec<_> = self
            .view
            .tab_hit_areas
            .iter()
            .enumerate()
            .filter(|(_, rect)| rect.width > 0)
            .collect();
        let (first_idx, first_rect) = *visible_tabs.first()?;
        let (last_idx, last_rect) = *visible_tabs.last()?;

        if self.on_tab_scroll_left_button(col, row) {
            return Some(0);
        }
        if self.on_tab_scroll_right_button(col, row) {
            return self
                .active
                .and_then(|idx| self.workspaces.get(idx))
                .map(|ws| ws.tabs.len());
        }

        let left_edge = if first_idx == 0 {
            first_rect.x
        } else {
            self.view.tab_scroll_left_hit_area.x + self.view.tab_scroll_left_hit_area.width
        };
        let right_edge = if self
            .active
            .and_then(|idx| self.workspaces.get(idx))
            .is_some_and(|ws| last_idx + 1 >= ws.tabs.len())
        {
            last_rect.x + last_rect.width
        } else {
            self.view.tab_scroll_right_hit_area.x.saturating_sub(1)
        };

        if col <= left_edge {
            return Some(first_idx);
        }
        if col >= right_edge {
            return Some(last_idx + 1);
        }

        for (idx, rect) in visible_tabs {
            let midpoint = rect.x + rect.width / 2;
            if col < midpoint {
                return Some(idx);
            }
            if col < rect.x + rect.width {
                return Some(idx + 1);
            }
        }

        Some(last_idx + 1)
    }

    pub(super) fn on_new_tab_button(&self, col: u16, row: u16) -> bool {
        let area = self.view.new_tab_hit_area;
        area.width > 0
            && row >= area.y
            && row < area.y + area.height
            && col >= area.x
            && col < area.x + area.width
    }

    pub(super) fn find_border_at(&self, col: u16, row: u16) -> Option<&SplitBorder> {
        self.view.split_borders.iter().find(|b| match b.direction {
            Direction::Horizontal if self.pane_borders && !self.pane_gaps => {
                col == b.pos && row >= b.area.y && row < b.area.y + b.area.height
            }
            Direction::Horizontal if self.pane_borders && self.pane_gaps => {
                row >= b.area.y
                    && row < b.area.y + b.area.height
                    && col >= b.pos.saturating_sub(1)
                    && col <= b.pos
            }
            Direction::Horizontal if !self.pane_borders && self.pane_gaps => {
                row >= b.area.y
                    && row < b.area.y + b.area.height
                    && b.pos.checked_sub(1).is_some_and(|gap_col| {
                        col == gap_col && self.pane_frame_at(col, row).is_none()
                    })
            }
            Direction::Vertical if self.pane_borders && !self.pane_gaps => {
                row == b.pos && col >= b.area.x && col < b.area.x + b.area.width
            }
            Direction::Vertical if self.pane_borders && self.pane_gaps => {
                col >= b.area.x
                    && col < b.area.x + b.area.width
                    && row >= b.pos.saturating_sub(1)
                    && row <= b.pos
            }
            Direction::Vertical if !self.pane_borders && self.pane_gaps => {
                col >= b.area.x
                    && col < b.area.x + b.area.width
                    && b.pos.checked_sub(1).is_some_and(|gap_row| {
                        row == gap_row && self.pane_frame_at(col, row).is_none()
                    })
            }
            _ => false,
        })
    }

    pub(super) fn pane_at(&self, col: u16, row: u16) -> Option<&PaneInfo> {
        self.view.pane_infos.iter().find(|p| {
            col >= p.inner_rect.x
                && col < p.inner_rect.x + p.inner_rect.width
                && row >= p.inner_rect.y
                && row < p.inner_rect.y + p.inner_rect.height
        })
    }

    pub(super) fn pane_mouse_target(&self, col: u16, row: u16) -> Option<&PaneInfo> {
        self.pane_at(col, row)
            .or_else(|| self.pane_frame_at(col, row))
    }

    fn chrome_press_pending(&self, source_id: crate::app::InputSourceId) -> bool {
        self.tab_presses.contains_key(&source_id) || self.workspace_presses.contains_key(&source_id)
    }

    fn chrome_drag_owned_by_other(&self, source_id: crate::app::InputSourceId) -> bool {
        self.drag.as_ref().is_some_and(|drag| {
            matches!(
                drag.target,
                DragTarget::WorkspaceReorder {
                    source_id: drag_source_id,
                    ..
                } | DragTarget::TabReorder {
                    source_id: drag_source_id,
                    ..
                } if drag_source_id != source_id
            )
        })
    }

    fn chrome_press_action(
        &mut self,
        workspace_press: Option<WorkspacePressState>,
        tab_press: Option<TabPressState>,
    ) -> Option<MouseAction> {
        if let Some(press) = workspace_press {
            self.mode = Mode::Terminal;
            return Some(MouseAction::FocusWorkspace {
                ws_idx: press.ws_idx,
            });
        }
        if let Some(press) = tab_press {
            if self.active == Some(press.ws_idx) {
                self.mode = Mode::Terminal;
                return Some(MouseAction::FocusTab {
                    tab_idx: press.tab_idx,
                });
            }
        }
        None
    }

    pub(crate) fn clear_chrome_gesture(&mut self, source_id: crate::app::InputSourceId) {
        if self.drag.as_ref().is_some_and(|drag| {
            matches!(
                drag.target,
                DragTarget::WorkspaceReorder {
                    source_id: drag_source_id,
                    ..
                } | DragTarget::TabReorder {
                    source_id: drag_source_id,
                    ..
                } if drag_source_id == source_id
            )
        }) {
            self.drag = None;
        }
        self.clear_chrome_press(source_id);
    }

    fn clear_chrome_press(&mut self, source_id: crate::app::InputSourceId) {
        self.tab_presses.remove(&source_id);
        self.workspace_presses.remove(&source_id);
    }

    fn mouse_pane_focus_action(&self, pane_id: crate::layout::PaneId) -> Option<MouseAction> {
        let ws_idx = self.active?;
        (self
            .workspaces
            .get(ws_idx)
            .and_then(crate::workspace::Workspace::focused_pane_id)
            != Some(pane_id))
        .then_some(MouseAction::FocusPane { ws_idx, pane_id })
    }

    pub(crate) fn pane_info_by_id(&self, pane_id: crate::layout::PaneId) -> Option<&PaneInfo> {
        self.view.pane_infos.iter().find(|info| info.id == pane_id)
    }

    pub(super) fn pane_frame_at(&self, col: u16, row: u16) -> Option<&PaneInfo> {
        self.view.pane_infos.iter().find(|p| {
            col >= p.rect.x
                && col < p.rect.x + p.rect.width
                && row >= p.rect.y
                && row < p.rect.y + p.rect.height
        })
    }

    pub(super) fn focus_pane(&mut self, pane_id: crate::layout::PaneId) {
        let _ = pane_id;
    }

    fn clickable_toast_at(&self, col: u16, row: u16) -> bool {
        self.toast
            .as_ref()
            .is_some_and(|toast| toast.target.is_some())
            && rect_contains(self.view.toast_hit_area, col, row)
    }

    #[cfg(test)]
    pub(crate) fn focus_toast_target(&mut self) {
        let Some(target) = self.toast.as_ref().and_then(|toast| toast.target.clone()) else {
            return;
        };
        let Some(ws_idx) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.id == target.workspace_id)
        else {
            return;
        };
        let Some(_tab_idx) = self.workspaces[ws_idx].find_tab_index_for_pane(target.pane_id) else {
            return;
        };

        self.focus_pane_in_workspace(ws_idx, target.pane_id);
        self.toast = None;
        self.settle_terminal_mode_after_focus();
    }

    pub(crate) fn scroll_pane_up(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
        lines: usize,
    ) {
        if let Some(ws_idx) = self.active {
            if let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
            {
                rt.scroll_up(lines);
            }
        }
    }

    pub(crate) fn scroll_pane_down(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
        lines: usize,
    ) {
        if let Some(ws_idx) = self.active {
            if let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
            {
                rt.scroll_down(lines);
            }
        }
    }

    pub(crate) fn pane_scroll_metrics(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
    ) -> Option<crate::pane::ScrollMetrics> {
        self.active
            .and_then(|i| self.runtime_for_pane_in_workspace(terminal_runtimes, i, pane_id))
            .and_then(crate::terminal::TerminalRuntime::scroll_metrics)
    }

    fn handle_right_click_passthrough(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        source_id: crate::app::InputSourceId,
        mouse: MouseEvent,
        in_sidebar: bool,
    ) -> bool {
        if let Some(gesture) = self.right_click_passthrough.clone() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Right)
                | MouseEventKind::Up(MouseButton::Right) => {
                    let forwarded_mouse =
                        self.strip_right_click_passthrough_modifiers(mouse, gesture.modifiers);
                    let _ = self.forward_pane_mouse_button(
                        terminal_runtimes,
                        &gesture.pane_info,
                        forwarded_mouse,
                    );
                    if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Right)) {
                        self.right_click_passthrough = None;
                    }
                    return true;
                }
                _ => {
                    self.right_click_passthrough = None;
                }
            }
        }

        if self.mode != Mode::Terminal
            || in_sidebar
            || !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Right))
        {
            return false;
        }

        let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() else {
            return false;
        };
        let configured_modifiers = self
            .right_click_passthrough_modifiers
            .filter(|modifiers| mouse.modifiers == *modifiers);
        let pane_passthrough = mouse.modifiers.is_empty()
            && self.active.is_some_and(|ws_idx| {
                self.workspaces
                    .get(ws_idx)
                    .and_then(|workspace| workspace.pane_state(info.id))
                    .is_some_and(|pane| pane.right_click_passthrough)
            });
        let Some(modifiers) = configured_modifiers
            .or_else(|| pane_passthrough.then(crossterm::event::KeyModifiers::empty))
        else {
            return false;
        };

        self.focus_pane(info.id);
        let forwarded_mouse = self.strip_right_click_passthrough_modifiers(mouse, modifiers);
        if !self.forward_pane_mouse_button(terminal_runtimes, &info, forwarded_mouse) {
            return false;
        }

        self.selection = None;
        self.selection_autoscroll = None;
        self.clear_chrome_press(source_id);
        self.drag = None;
        self.context_menu = None;
        self.right_click_passthrough = Some(RightClickPassthroughGesture {
            pane_info: info,
            modifiers,
        });
        true
    }

    fn strip_right_click_passthrough_modifiers(
        &self,
        mouse: MouseEvent,
        modifiers: crossterm::event::KeyModifiers,
    ) -> MouseEvent {
        MouseEvent {
            modifiers: mouse.modifiers.difference(modifiers),
            ..mouse
        }
    }

    pub(super) fn handle_terminal_wheel(
        &mut self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        mouse: MouseEvent,
    ) {
        let lines_per_notch = self.mouse_scroll_lines;

        if let Some(info) = self.pane_at(mouse.column, mouse.row).cloned() {
            self.focus_pane(info.id);
            if self.forward_pane_wheel(terminal_runtimes, &info, mouse) {
                return;
            }
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_pane_up(terminal_runtimes, info.id, lines_per_notch)
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_pane_down(terminal_runtimes, info.id, lines_per_notch)
                }
                _ => {}
            }
            return;
        }

        if let Some(info) = self.pane_frame_at(mouse.column, mouse.row).cloned() {
            self.focus_pane(info.id);
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.scroll_pane_up(terminal_runtimes, info.id, lines_per_notch)
                }
                MouseEventKind::ScrollDown => {
                    self.scroll_pane_down(terminal_runtimes, info.id, lines_per_notch)
                }
                _ => {}
            }
            return;
        }

        if let Some(ws_idx) = self.active {
            if let Some(rt) = self.focused_runtime_in_workspace(terminal_runtimes, ws_idx) {
                match mouse.kind {
                    MouseEventKind::ScrollUp => rt.scroll_up(lines_per_notch),
                    MouseEventKind::ScrollDown => rt.scroll_down(lines_per_notch),
                    _ => {}
                }
            }
        }
    }

    fn pane_mouse_position(
        &self,
        runtime: &crate::terminal::TerminalRuntime,
        inner: Rect,
        mouse: MouseEvent,
    ) -> Option<crate::input::mouse::Position> {
        let column = mouse.column.saturating_sub(inner.x);
        let row = mouse.row.saturating_sub(inner.y);
        let cell = crate::input::mouse::Position::Cell { column, row };
        let Some(host) = self.host_mouse_pixels else {
            return Some(cell);
        };
        let wants_pixels = runtime.sgr_pixel_mouse_enabled();
        if !wants_pixels {
            return Some(cell);
        }
        let Some((width_px, height_px)) = runtime.pixel_size() else {
            return Some(cell);
        };
        Some(
            host.pane_position(inner, width_px, height_px)
                .unwrap_or(cell),
        )
    }

    pub(super) fn forward_pane_mouse_button(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        info: &PaneInfo,
        mouse: MouseEvent,
    ) -> bool {
        let Some(ws_idx) = self.active else {
            return false;
        };
        let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id)
        else {
            return false;
        };
        let Some(position) = self.pane_mouse_position(rt, info.inner_rect, mouse) else {
            return false;
        };
        let Some(bytes) = rt.encode_mouse_button(mouse.kind, position, mouse.modifiers) else {
            return false;
        };
        rt.scroll_reset();
        if let Err(err) = rt.try_send_bytes(Bytes::from(bytes)) {
            warn!(pane = info.id.raw(), err = %err, kind = ?mouse.kind, "failed to forward mouse button event");
        }
        true
    }

    pub(super) fn forward_pane_mouse_motion(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        info: &PaneInfo,
        mouse: MouseEvent,
    ) -> bool {
        let Some(ws_idx) = self.active else {
            return false;
        };
        let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id)
        else {
            return false;
        };
        let Some(position) = self.pane_mouse_position(rt, info.inner_rect, mouse) else {
            return false;
        };
        let Some(bytes) = rt.encode_mouse_motion(mouse.kind, position, mouse.modifiers) else {
            return false;
        };
        if let Err(err) = rt.try_send_bytes(Bytes::from(bytes)) {
            warn!(pane = info.id.raw(), err = %err, kind = ?mouse.kind, "failed to forward mouse motion event");
        }
        true
    }

    fn forward_pane_reported_wheel(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        info: &PaneInfo,
        mouse: MouseEvent,
    ) -> bool {
        let Some(ws_idx) = self.active else {
            return false;
        };
        let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id)
        else {
            return false;
        };
        if rt.wheel_routing() != Some(crate::pane::WheelRouting::MouseReport) {
            return false;
        }
        rt.scroll_reset();
        let Some(position) = self.pane_mouse_position(rt, info.inner_rect, mouse) else {
            return false;
        };
        let Some(bytes) = rt.encode_mouse_wheel(mouse.kind, position, mouse.modifiers) else {
            warn!(pane = info.id.raw(), kind = ?mouse.kind, "failed to encode mouse wheel event");
            return true;
        };
        if let Err(err) = rt.try_send_bytes(Bytes::from(bytes)) {
            warn!(pane = info.id.raw(), err = %err, "failed to forward mouse wheel event");
        }
        true
    }

    pub(super) fn forward_pane_wheel(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        info: &PaneInfo,
        mouse: MouseEvent,
    ) -> bool {
        let Some(ws_idx) = self.active else {
            return false;
        };
        let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id)
        else {
            return false;
        };
        match rt.wheel_routing() {
            Some(crate::pane::WheelRouting::HostScroll) | None => false,
            Some(crate::pane::WheelRouting::MouseReport) => {
                self.forward_pane_reported_wheel(terminal_runtimes, info, mouse)
            }
            Some(crate::pane::WheelRouting::AlternateScroll) => {
                rt.scroll_reset();
                let Some(bytes) = rt.encode_alternate_scroll(mouse.kind) else {
                    return true;
                };
                if let Err(err) = rt.try_send_bytes(Bytes::from(bytes)) {
                    warn!(pane = info.id.raw(), err = %err, "failed to forward alternate-scroll key");
                }
                true
            }
        }
    }

    pub(super) fn set_pane_scroll_offset(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
        offset_from_bottom: usize,
    ) {
        for ws_idx in 0..self.workspaces.len() {
            let Some(rt) = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)
            else {
                continue;
            };
            rt.set_scroll_offset_from_bottom(offset_from_bottom);
            return;
        }
    }

    pub(super) fn scrollbar_target_at(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        col: u16,
        row: u16,
    ) -> Option<(crate::layout::PaneId, ScrollbarClickTarget)> {
        let ws_idx = self.active?;
        let info = self.view.pane_infos.iter().find(|info| {
            crate::ui::pane_scrollbar_rect(info).is_some_and(|track| {
                col >= track.x
                    && col < track.x + track.width
                    && row >= track.y
                    && row < track.y + track.height
            })
        })?;
        let rt = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id)?;
        let metrics = rt.scroll_metrics()?;
        if metrics.max_offset_from_bottom == 0 {
            return None;
        }
        let track = crate::ui::pane_scrollbar_rect(info)?;
        if let Some(grab_row_offset) = crate::ui::scrollbar_thumb_grab_offset(metrics, track, row) {
            Some((info.id, ScrollbarClickTarget::Thumb { grab_row_offset }))
        } else {
            Some((
                info.id,
                ScrollbarClickTarget::Track {
                    offset_from_bottom: crate::ui::scrollbar_offset_from_row(metrics, track, row),
                },
            ))
        }
    }

    pub(super) fn scrollbar_offset_for_pane_row(
        &self,
        terminal_runtimes: &TerminalRuntimeRegistry,
        pane_id: crate::layout::PaneId,
        row: u16,
        grab_row_offset: u16,
    ) -> Option<usize> {
        let ws_idx = self.active?;
        let info = self
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)?;
        let track = crate::ui::pane_scrollbar_rect(info)?;
        let rt = self.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, pane_id)?;
        let metrics = rt.scroll_metrics()?;
        if metrics.max_offset_from_bottom == 0 {
            return None;
        }
        Some(crate::ui::scrollbar_offset_from_drag_row(
            metrics,
            track,
            row,
            grab_row_offset,
        ))
    }

    /// Dispatches a Project-view row click resolved by
    /// `sidebar::project_row_target_at` against the geometry pass's own
    /// `ProjectRowHitArea`s — it never re-derives what a row means from the
    /// mouse position itself.
    fn handle_project_row_click(
        &mut self,
        target: crate::app::state::ProjectRowTarget,
    ) -> Option<MouseAction> {
        use crate::app::state::ProjectRowTarget;
        match target {
            ProjectRowTarget::Project { collapse_key }
            | ProjectRowTarget::Band { collapse_key }
            | ProjectRowTarget::Section { collapse_key, .. } => {
                self.toggle_project_row_collapse(collapse_key);
                None
            }
            ProjectRowTarget::OpenWorktree { checkout_key } => {
                self.request_new_workspace_cwd = Some(std::path::PathBuf::from(checkout_key));
                None
            }
            ProjectRowTarget::Pane { ws_idx, pane_id } => {
                let pane_id = self
                    .workspaces
                    .get(ws_idx)
                    .and_then(|ws| pane_id_from_public_number(ws, &pane_id))?;
                self.mode = Mode::Terminal;
                Some(MouseAction::FocusPane { ws_idx, pane_id })
            }
            // COMMANDS rows launch into the worktree's representative
            // workspace (bora-55c.3), resolved from the tick-refreshed
            // command cache. CHECKS/TODOS/NOTES rows have no action yet.
            ProjectRowTarget::SectionItem {
                kind,
                label,
                ws_idx,
            } => {
                // Compares by wire name rather than descriptor identity —
                // deliberate (bora-by6 G6): click behaviour is not yet a
                // declared field on `SectionDescriptor`, so this stays the
                // same "COMMANDS only" wildcard the closed enum had. An
                // `on_click` descriptor field is the natural next step once
                // a non-COMMANDS band needs one.
                if kind.wire_name == "commands" {
                    if let Some(ws_idx) = ws_idx {
                        self.pending_bora_command = self.section_command_launch(ws_idx, &label);
                    }
                }
                None
            }
            // A PR row in the project-level PULL REQUESTS band opens the PR
            // in a new worktree — the same destination
            // `ContextMenuKind::RepoPr`'s "Open in worktree" reaches, set
            // through the same `request_open_pr_worktree` field, so the
            // sidebar row and the right panel's right-click cannot drift.
            ProjectRowTarget::OpenPr { ws_idx, number } => {
                self.request_open_pr_worktree = Some((ws_idx, number));
                None
            }
            // T4 (bora-79l, P3): the SectionRow header's "+" — defer, like
            // every App-owned action, resolving nothing here. The pair is
            // the section's branch-group identity, re-resolved to a source
            // workspace at drain time (`start_section_worktree_create`).
            ProjectRowTarget::SectionNew {
                repo_identity,
                branch,
            } => {
                self.request_section_worktree_create = Some((repo_identity, branch));
                None
            }
        }
    }

    /// Toggles one Project-view collapse key. Collapsing/expanding reflows
    /// every row below it without changing the sidebar's own outer
    /// dimensions, so — unlike a resize — the dimension-keyed full-repaint
    /// heuristics never see it; the layout change itself must ask for a
    /// full repaint (AGENTS.md, learned 2026-08-13) or a client keeps
    /// stale rows under the ones that just moved.
    fn toggle_project_row_collapse(&mut self, key: String) {
        if self.collapsed_space_keys.contains(&key) {
            self.collapsed_space_keys.remove(&key);
        } else {
            self.collapsed_space_keys.insert(key);
        }
        self.mark_session_dirty();
        self.request_full_repaint();
    }
}

#[cfg(test)]
pub(super) fn wheel_routing(input_state: crate::pane::InputState) -> WheelRouting {
    if input_state.mouse_protocol_mode.reporting_enabled() {
        WheelRouting::MouseReport
    } else if input_state.alternate_screen && input_state.mouse_alternate_scroll {
        WheelRouting::AlternateScroll
    } else {
        WheelRouting::HostScroll
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn apply_scroll(scroll: &mut usize, delta: i16, max_scroll: usize) {
    if delta.is_negative() {
        *scroll = scroll.saturating_sub(delta.unsigned_abs() as usize);
    } else {
        *scroll = scroll.saturating_add(delta as usize).min(max_scroll);
    }
}

/// Resolves a `ProjectRowTarget::Pane`'s `pane_id` — the row's public pane
/// number (e.g. `"p1"`, or the full `"w28p1"` compact id — see
/// `workspace::public_pane_id_for_number`) — against `ws`'s live panes.
/// Never reconstructs a raw `layout::PaneId`: that type has no public
/// constructor from an arbitrary value outside `layout.rs`. The public
/// pane number is the one stable identifier already shared by chat
/// addressing and the socket API, so this reuses it rather than inventing
/// a second convention.
fn pane_id_from_public_number(
    ws: &crate::workspace::Workspace,
    raw: &str,
) -> Option<crate::layout::PaneId> {
    let encoded = raw.rsplit('p').next().unwrap_or(raw);
    let pane_number = crate::workspace::decode_public_number(encoded)?;
    ws.public_pane_numbers
        .iter()
        .find_map(|(id, number)| (*number == pane_number).then_some(*id))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
    use ratatui::layout::{Direction, Rect};

    use super::super::{
        app_for_mouse_test, capture_snapshot, mouse, numbered_lines_bytes, root_layout_ratio,
        unique_temp_path,
    };
    use super::*;
    use crate::app::input::modal::handle_context_menu_key;
    use crate::{
        app::state::{
            build_context_menu_items, ContextMenuKind, ContextMenuState, MenuListState, Mode,
            ProjectRowHitArea, ProjectRowTarget, ViewLayout,
        },
        detect::{Agent, AgentState},
        workspace::Workspace,
    };

    #[test]
    fn clicking_a_project_row_toggles_collapse_and_forces_full_repaint() {
        let mut app = app_for_mouse_test();
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let list_area = app.state.workspace_list_rect();
        // Offset row (not the list's first line) — a dispatcher that
        // re-derives position instead of trusting the area would miss it.
        let row_rect = Rect::new(list_area.x, list_area.y + 3, list_area.width, 1);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: row_rect,
            target: ProjectRowTarget::Project {
                collapse_key: "project:cnb".into(),
            },
        }];
        app.state.force_full_repaint = false;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            row_rect.x,
            row_rect.y,
        ));

        assert!(app.state.collapsed_space_keys.contains("project:cnb"));
        assert!(
            app.state.force_full_repaint,
            "collapsing a project row must force a full repaint (AGENTS.md 2026-08-13)"
        );

        app.state.force_full_repaint = false;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            row_rect.x,
            row_rect.y,
        ));
        assert!(!app.state.collapsed_space_keys.contains("project:cnb"));
        assert!(
            app.state.force_full_repaint,
            "expanding it back must repaint too"
        );
    }

    #[test]
    fn view_mode_toggle_click_cycles_view_mode_and_resets_workspace_scroll() {
        // Restores 7bb8133b's removed click target: the toggle rect is
        // narrow and right-aligned on the workspace list's first row, so
        // a click just outside it (to the left) must NOT cycle — that
        // cell still belongs to the drag "drop above first card" row.
        let mut app = app_for_mouse_test();
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let (ws_area, _) = crate::ui::expanded_sidebar_sections(
            app.state.view.sidebar_rect,
            app.state.sidebar_section_split,
        );
        let toggle_rect = crate::ui::view_mode_toggle_rect(ws_area, app.state.view_mode);
        assert!(toggle_rect.width > 0, "toggle rect must be non-empty");
        assert!(toggle_rect.x > 0, "toggle rect must not start at column 0");

        let before = app.state.view_mode;
        app.state.workspace_scroll = 3;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle_rect.x,
            toggle_rect.y,
        ));
        assert_eq!(app.state.view_mode, before.cycle());
        assert_eq!(app.state.workspace_scroll, 0);
        assert!(
            app.state.force_full_repaint,
            "a view-mode change reflows the whole list without changing terminal \
             dimensions and must force a full repaint (AGENTS.md)"
        );

        app.state.workspace_scroll = 3;
        app.state.force_full_repaint = false;
        // Recompute: the toggle label's width differs per mode ("repo"
        // vs "project"), so the rect itself moves after the cycle above.
        let (ws_area, _) = crate::ui::expanded_sidebar_sections(
            app.state.view.sidebar_rect,
            app.state.sidebar_section_split,
        );
        let toggle_rect = crate::ui::view_mode_toggle_rect(ws_area, app.state.view_mode);
        assert!(toggle_rect.x > 0, "toggle rect must not start at column 0");
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            toggle_rect.x - 1,
            toggle_rect.y,
        ));
        assert_eq!(
            app.state.view_mode,
            before.cycle(),
            "a click just left of the toggle rect must not cycle again"
        );
        assert_eq!(
            app.state.workspace_scroll, 3,
            "a click outside the toggle must not touch workspace_scroll"
        );
    }

    #[test]
    fn clicking_an_open_worktree_row_requests_a_new_workspace_at_its_checkout_path() {
        let mut app = app_for_mouse_test();
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let list_area = app.state.workspace_list_rect();
        let row_rect = Rect::new(list_area.x, list_area.y + 1, list_area.width, 1);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: row_rect,
            target: ProjectRowTarget::OpenWorktree {
                checkout_key: "/repo/cnb-worktree".into(),
            },
        }];

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            row_rect.x,
            row_rect.y,
        ));

        assert_eq!(
            app.state.request_new_workspace_cwd,
            Some(std::path::PathBuf::from("/repo/cnb-worktree"))
        );
    }

    #[test]
    fn clicking_a_project_pane_row_focuses_the_pane_by_its_public_number() {
        let mut app = app_for_mouse_test();
        let ws = Workspace::test_new("multi-pane");
        let root_pane = ws.tabs[0].root_pane;
        app.state.workspaces = vec![ws];
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let list_area = app.state.workspace_list_rect();
        let row_rect = Rect::new(list_area.x, list_area.y + 2, list_area.width, 1);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: row_rect,
            target: ProjectRowTarget::Pane {
                ws_idx: 0,
                pane_id: "p1".into(),
            },
        }];

        let action = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                row_rect.x,
                row_rect.y,
            ),
        );

        let Some(MouseAction::FocusPane { ws_idx, pane_id }) = action else {
            panic!("expected FocusPane action");
        };
        assert_eq!(ws_idx, 0);
        assert_eq!(
            pane_id, root_pane,
            "the workspace's only (root, public number 1) pane must be the focus target"
        );
    }

    #[test]
    fn tab_click_survives_stray_drag_report_off_the_tab_bar() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(None);
        ws.active_tab = 1;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        let area = Rect::new(0, 0, 106, 20);
        crate::ui::compute_view(&mut app.state, area);

        let first_tab = app.state.view.tab_hit_areas[0];
        let press_col = first_tab.x + 1;
        let stray_row = area.height - 1;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            press_col,
            first_tab.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            press_col,
            stray_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            press_col,
            stray_row,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, 0);
    }

    #[test]
    fn workspace_click_survives_stray_drag_report_off_the_workspace_list() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("first"), Workspace::test_new("second")];
        app.state.active = Some(1);
        app.state.selected = 1;
        let area = Rect::new(0, 0, 106, 20);
        crate::ui::compute_view(&mut app.state, area);

        let first_workspace = app.state.view.workspace_card_areas[0];
        let press_col = first_workspace.rect.x + 1;
        let press_row = first_workspace.rect.y;
        let stray_row = area.height - 1;
        assert!(app.state.workspace_drop_index_at_row(stray_row).is_none());

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            press_col,
            press_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            press_col,
            stray_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            press_col,
            stray_row,
        ));

        assert_eq!(app.state.active, Some(0));
    }

    #[test]
    fn concurrent_input_sources_keep_their_tab_clicks() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(None);
        ws.test_add_tab(None);
        ws.active_tab = 2;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let first_tab = app.state.view.tab_hit_areas[0];
        let second_tab = app.state.view.tab_hit_areas[1];
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                first_tab.x + 1,
                first_tab.y,
            ),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                second_tab.x + 1,
                second_tab.y,
            ),
        );

        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                first_tab.x + 1,
                first_tab.y,
            ),
        );
        assert_eq!(app.state.workspaces[0].active_tab, 0);

        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                second_tab.x + 1,
                second_tab.y,
            ),
        );
        assert_eq!(app.state.workspaces[0].active_tab, 1);
    }

    #[test]
    fn concurrent_input_sources_keep_their_workspace_clicks() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![
            Workspace::test_new("first"),
            Workspace::test_new("second"),
            Workspace::test_new("third"),
        ];
        app.state.active = Some(2);
        app.state.selected = 2;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let first = app.state.view.workspace_card_areas[0].rect;
        let second = app.state.view.workspace_card_areas[1].rect;
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                first.x + 1,
                first.y,
            ),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                second.x + 1,
                second.y,
            ),
        );

        app.handle_mouse_from_input_source(
            41,
            mouse(MouseEventKind::Up(MouseButton::Left), first.x + 1, first.y),
        );
        assert_eq!(app.state.active, Some(0));

        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                second.x + 1,
                second.y,
            ),
        );
        assert_eq!(app.state.active, Some(1));
    }

    #[test]
    fn tab_click_completes_while_other_source_reorders() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(None);
        ws.test_add_tab(None);
        ws.active_tab = 2;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let first_tab = app.state.view.tab_hit_areas[0];
        let second_tab = app.state.view.tab_hit_areas[1];
        let last_tab = app.state.view.tab_hit_areas[2];
        let drop_col = last_tab.x + last_tab.width;
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                first_tab.x + 1,
                first_tab.y,
            ),
        );
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                drop_col,
                first_tab.y,
            ),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                second_tab.x + 1,
                second_tab.y,
            ),
        );

        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                second_tab.x + 1,
                second_tab.y,
            ),
        );
        assert_eq!(app.state.workspaces[0].active_tab, 1);
        assert!(matches!(
            app.state.drag.as_ref().map(|drag| &drag.target),
            Some(DragTarget::TabReorder { source_id: 41, .. })
        ));

        app.handle_mouse_from_input_source(
            41,
            mouse(MouseEventKind::Up(MouseButton::Left), drop_col, first_tab.y),
        );
        assert!(app.state.drag.is_none());
    }

    #[test]
    fn releasing_input_source_clears_only_its_pending_tab_click() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(None);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let first_tab = app.state.view.tab_hit_areas[0];
        let second_tab = app.state.view.tab_hit_areas[1];
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                first_tab.x + 1,
                first_tab.y,
            ),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                second_tab.x + 1,
                second_tab.y,
            ),
        );

        app.clear_input_source(41);

        assert!(!app.state.tab_presses.contains_key(&41));
        assert!(app.state.tab_presses.contains_key(&42));
    }

    #[tokio::test]
    async fn other_input_source_pane_gesture_is_not_swallowed_by_chrome_press() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(None);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        let area = Rect::new(0, 0, 106, 20);
        crate::ui::compute_view(&mut app.state, area);

        let info = app.state.view.pane_infos[0].clone();
        let pane_id = info.id;
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1000h\x1b[?1006h",
                4,
            );
        app.state.insert_test_runtime(pane_id, runtime);
        crate::ui::compute_view(&mut app.state, area);

        let first_tab = app.state.view.tab_hit_areas[0];
        let pane_col = info.inner_rect.x;
        let pane_row = info.inner_rect.y;
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                first_tab.x + 1,
                first_tab.y,
            ),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(MouseEventKind::Down(MouseButton::Left), pane_col, pane_row),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(MouseEventKind::Up(MouseButton::Left), pane_col, pane_row),
        );

        assert_eq!(
            input_rx.try_recv().expect("other source mouse down"),
            Bytes::from_static(b"\x1b[<0;1;1M")
        );
        assert_eq!(
            input_rx.try_recv().expect("other source mouse up"),
            Bytes::from_static(b"\x1b[<0;1;1m")
        );
    }

    #[test]
    fn other_input_source_cannot_release_tab_reorder() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(Some("second"));
        ws.test_add_tab(Some("third"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let source = app.state.view.tab_hit_areas[0];
        let target = app.state.view.tab_hit_areas[2];
        let drop_col = target.x + target.width;
        app.handle_mouse_from_input_source(
            41,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                source.x + 1,
                source.y,
            ),
        );
        app.handle_mouse_from_input_source(
            41,
            mouse(MouseEventKind::Drag(MouseButton::Left), drop_col, source.y),
        );
        app.handle_mouse_from_input_source(
            42,
            mouse(MouseEventKind::Up(MouseButton::Left), drop_col, source.y),
        );

        assert!(app.state.drag.is_some());
        assert_eq!(app.state.workspaces[0].tabs[0].custom_name, None);

        app.handle_mouse_from_input_source(
            41,
            mouse(MouseEventKind::Up(MouseButton::Left), drop_col, source.y),
        );

        assert!(app.state.drag.is_none());
        assert_eq!(app.state.workspaces[0].tabs[2].custom_name.as_deref(), None);
    }

    #[tokio::test]
    async fn tab_click_survives_stray_drag_report_into_a_mouse_reporting_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        ws.test_add_tab(None);
        ws.active_tab = 1;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        let area = Rect::new(0, 0, 106, 20);
        crate::ui::compute_view(&mut app.state, area);

        let info = app
            .state
            .view
            .pane_infos
            .first()
            .cloned()
            .expect("visible pane");
        app.state.insert_test_runtime(
            info.id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                info.inner_rect.width.max(1),
                info.inner_rect.height.max(1),
                b"\x1b[?1002h",
            ),
        );
        crate::ui::compute_view(&mut app.state, area);

        let first_tab = app.state.view.tab_hit_areas[0];
        let press_col = first_tab.x + 1;
        let stray_row = info.inner_rect.bottom().saturating_sub(1);
        assert!(
            app.state.pane_mouse_target(press_col, stray_row).is_some(),
            "stray coordinates must land on the pane for this to test anything"
        );

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            press_col,
            first_tab.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            press_col,
            stray_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            press_col,
            stray_row,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, 0);
    }

    fn mark_worktree_space_member(workspace: &mut Workspace, ws_idx: usize, key: &str) {
        workspace.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: key.into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: format!("/repo/worktree-{ws_idx}").into(),
            is_linked_worktree: ws_idx != 0,
        });
    }

    #[tokio::test]
    async fn terminal_wheel_uses_configured_mouse_scroll_lines() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        ws.tabs[0].runtimes.insert(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                16 * 1024,
                &numbered_lines_bytes(64),
            ),
        );

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;
        app.state.mouse_scroll_lines = 7;

        app.handle_mouse(mouse(
            MouseEventKind::ScrollUp,
            info.inner_rect.x + 1,
            info.inner_rect.y + 1,
        ));

        let metrics = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .and_then(crate::terminal::TerminalRuntime::scroll_metrics)
            .expect("scroll metrics after wheel");
        assert_eq!(metrics.offset_from_bottom, 7);
    }

    #[tokio::test]
    async fn mouse_dispatcher_forwards_horizontal_wheel_to_mouse_reporting_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1000h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;
        assert!(
            app.state.mouse_capture,
            "reproduction must use the default Herdr mouse dispatcher"
        );

        let outer_column = info.inner_rect.x + 2;
        let outer_row = info.inner_rect.y + 3;
        for (button, expected_kind, ingress) in [
            (66, MouseEventKind::ScrollLeft, "monolithic"),
            (67, MouseEventKind::ScrollRight, "headless"),
        ] {
            let input = format!("\x1b[<{button};{};{}M", outer_column + 1, outer_row + 1);
            let mut events = crate::raw_input::parse_raw_input_bytes_sync(input.as_bytes());
            let event = events
                .pop()
                .expect("horizontal SGR wheel input should parse");
            let crate::raw_input::RawInputEvent::Mouse(mouse) = &event else {
                panic!("expected parsed mouse event");
            };
            assert!(events.is_empty(), "expected one parsed mouse event");
            assert_eq!(mouse.kind, expected_kind);

            if ingress == "monolithic" {
                assert!(app.handle_raw_input_event(event).await);
            } else {
                app.route_client_events(vec![event], false);
            }

            assert_eq!(
                input_rx
                    .try_recv()
                    .expect("horizontal wheel should reach pane"),
                Bytes::from(format!("\x1b[<{button};3;4M"))
            );
        }
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn horizontal_wheel_stays_inert_for_non_mouse_reporting_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"",
                1,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;

        let input = format!(
            "\x1b[<66;{};{}M",
            info.inner_rect.x + 3,
            info.inner_rect.y + 4
        );
        let event = crate::raw_input::parse_raw_input_bytes_sync(input.as_bytes())
            .pop()
            .expect("horizontal SGR wheel input should parse");

        assert!(app.handle_raw_input_event(event).await);

        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pane_right_click_passthrough_is_isolated() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let passthrough_pane = ws.tabs[0].root_pane;
        let default_pane = ws.test_split(Direction::Horizontal);
        ws.pane_state_mut(passthrough_pane)
            .unwrap()
            .right_click_passthrough = true;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let passthrough_info = app.state.pane_info_by_id(passthrough_pane).unwrap().clone();
        let default_info = app.state.pane_info_by_id(default_pane).unwrap().clone();
        let (passthrough_runtime, mut passthrough_input) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                passthrough_info.inner_rect.width,
                passthrough_info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        let (default_runtime, mut default_input) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                default_info.inner_rect.width,
                default_info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        app.state
            .insert_test_runtime(passthrough_pane, passthrough_runtime);
        app.state.insert_test_runtime(default_pane, default_runtime);

        let col = passthrough_info.inner_rect.x + 2;
        let row = passthrough_info.inner_rect.y + 3;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), col, row));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.context_menu.is_none());
        assert_eq!(
            passthrough_input.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[<2;3;4M")
        );

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            default_info.inner_rect.x + 2,
            default_info.inner_rect.y + 3,
        ));

        assert!(default_input.try_recv().is_err());
        assert!(matches!(
            app.state.context_menu.as_ref().map(|menu| &menu.kind),
            Some(ContextMenuKind::Pane { pane_id, .. }) if *pane_id == default_pane
        ));
    }

    #[tokio::test]
    async fn pane_right_click_passthrough_falls_back_when_mouse_reporting_is_off() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        ws.pane_state_mut(pane_id).unwrap().right_click_passthrough = true;
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let info = app.state.pane_info_by_id(pane_id).unwrap().clone();
        app.state.insert_test_runtime(
            pane_id,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                b"",
            ),
        );

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            info.inner_rect.x + 2,
            info.inner_rect.y + 3,
        ));

        assert_eq!(app.state.mode, Mode::ContextMenu);
        assert!(app.state.context_menu.is_some());
    }

    #[tokio::test]
    async fn configured_right_click_passthrough_forwards_gesture_outside_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;
        app.state.right_click_passthrough_modifiers = Some(KeyModifiers::CONTROL);

        let col = info.inner_rect.x + 2;
        let row = info.inner_rect.y + 3;
        app.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(MouseEventKind::Down(MouseButton::Right), col, row)
        });
        app.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(MouseEventKind::Drag(MouseButton::Right), 0, 0)
        });
        app.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(MouseEventKind::Up(MouseButton::Right), 0, 0)
        });

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.context_menu.is_none());
        assert!(app.state.right_click_passthrough.is_none());
        assert_eq!(
            input_rx.try_recv().expect("forwarded right mouse down"),
            Bytes::from_static(b"\x1b[<2;3;4M")
        );
        assert_eq!(
            input_rx.try_recv().expect("forwarded right mouse drag"),
            Bytes::from_static(b"\x1b[<34;1;1M")
        );
        assert_eq!(
            input_rx.try_recv().expect("forwarded right mouse up"),
            Bytes::from_static(b"\x1b[<2;1;1m")
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn captured_left_press_focuses_target_before_forwarding() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let source = ws.tabs[0].root_pane;
        let target = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(source);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let info = app
            .state
            .pane_info_by_id(target)
            .expect("target pane info")
            .clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        app.state.insert_test_runtime(target, runtime);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            info.inner_rect.x + 1,
            info.inner_rect.y + 1,
        ));

        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(target));
        assert_eq!(
            input_rx.try_recv().expect("forwarded captured left press"),
            Bytes::from_static(b"\x1b[<0;2;2M")
        );
    }

    #[tokio::test]
    async fn pane_mouse_only_forwards_moved_events_for_any_motion_apps() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1003h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;

        app.state.handle_pane_mouse_only(
            &app.terminal_runtimes,
            mouse(
                MouseEventKind::Moved,
                info.inner_rect.x + 2,
                info.inner_rect.y + 3,
            ),
        );

        assert_eq!(
            input_rx.try_recv().expect("forwarded mouse motion"),
            Bytes::from_static(b"\x1b[<35;3;4M")
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pane_mouse_motion_uses_computed_inner_rect_offsets() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                18,
                0,
                b"\x1b[?1003h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let info = app.state.view.pane_infos[0].clone();
        assert!(info.inner_rect.x > 0, "sidebar offset should be present");
        assert!(info.inner_rect.y > 0, "tab bar offset should be present");

        app.state.handle_pane_mouse_only(
            &app.terminal_runtimes,
            mouse(
                MouseEventKind::Moved,
                info.inner_rect.x + 2,
                info.inner_rect.y + 3,
            ),
        );

        assert_eq!(
            input_rx.try_recv().expect("forwarded mouse motion"),
            Bytes::from_static(b"\x1b[<35;3;4M")
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn ordinary_cell_mouse_downgrades_pixel_mode_to_cell_coordinates() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                18,
                0,
                b"\x1b[?1003h\x1b[?1006h\x1b[?1016h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.host_cell_size = crate::kitty_graphics::HostCellSize {
            width_px: 10,
            height_px: 20,
        };
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let info = app.state.view.pane_infos[0].clone();
        app.state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .unwrap()
            .resize(info.inner_rect.height, info.inner_rect.width, 10, 20);
        assert!(info.inner_rect.x > 0, "sidebar offset should be present");
        assert!(info.inner_rect.y > 0, "tab bar offset should be present");

        app.handle_mouse(mouse(
            MouseEventKind::Moved,
            info.inner_rect.x + 2,
            info.inner_rect.y + 3,
        ));

        assert_eq!(
            input_rx.try_recv().expect("forwarded mouse motion"),
            Bytes::from_static(b"\x1b[<35;3;4M")
        );
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn dedicated_client_pixel_mouse_preserves_subcell_position() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                18,
                0,
                b"\x1b[?1003h\x1b[?1006h\x1b[?1016h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.mouse_capture = false;
        app.state.host_cell_size = crate::kitty_graphics::HostCellSize {
            width_px: 10,
            height_px: 20,
        };
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let inner = app.state.view.pane_infos[0].inner_rect;
        let geometry = crate::input::mouse::HostGeometry::new(106, 20, 1_060, 400).unwrap();
        let x = u32::from(inner.x + 2) * 10 + 8;
        let y = u32::from(inner.y + 3) * 20 + 9;
        let report = format!("\x1b[<35;{x};{y}M");
        app.state.host_mouse_pixels = Some(crate::input::mouse::HostPixels { x, y, geometry });
        let runtime = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .unwrap();
        assert_eq!(runtime.pixel_size(), None);
        assert_eq!(
            app.state.pane_mouse_position(
                runtime,
                inner,
                mouse(MouseEventKind::Moved, inner.x + 2, inner.y + 3),
            ),
            Some(crate::input::mouse::Position::Cell { column: 2, row: 3 })
        );
        runtime.resize(inner.height, inner.width, 10, 20);
        let runtime = app
            .state
            .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
            .unwrap();
        assert_eq!(
            runtime.pixel_size(),
            Some((u32::from(inner.width) * 10, u32::from(inner.height) * 20))
        );
        assert_eq!(
            app.state.pane_mouse_position(
                runtime,
                inner,
                mouse(MouseEventKind::Moved, inner.x + 2, inner.y + 3),
            ),
            Some(crate::input::mouse::Position::Pixels { x: 28, y: 69 })
        );
        app.state.host_mouse_pixels = None;

        assert!(app.route_client_pixel_mouse(7, report.as_bytes(), geometry));
        assert_eq!(
            input_rx.try_recv().expect("forwarded exact mouse motion"),
            Bytes::from_static(b"\x1b[<35;28;69M")
        );
        assert!(input_rx.try_recv().is_err());
        assert!(app.state.host_mouse_pixels.is_none());
    }

    #[tokio::test]
    async fn mouse_dispatcher_does_not_forward_motion_behind_herdr_modes() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80,
                18,
                0,
                b"\x1b[?1003h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Navigate;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let info = app.state.view.pane_infos[0].clone();

        app.handle_mouse(mouse(
            MouseEventKind::Moved,
            info.inner_rect.x + 2,
            info.inner_rect.y + 3,
        ));

        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn unset_right_click_passthrough_keeps_modified_right_click_as_herdr_menu() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;
        app.state.right_click_passthrough_modifiers = None;

        app.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(
                MouseEventKind::Down(MouseButton::Right),
                info.inner_rect.x + 2,
                info.inner_rect.y + 3,
            )
        });

        assert_eq!(app.state.mode, Mode::ContextMenu);
        assert!(app.state.context_menu.is_some());
        assert!(app.state.right_click_passthrough.is_none());
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn pane_right_click_keeps_focus_and_swap_menu_swaps_with_focused_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let source = ws.tabs[0].root_pane;
        let target = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(source);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 100, 20));
        let target_info = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == target)
            .expect("target pane info")
            .clone();
        let source_rect_before = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == source)
            .expect("source pane info")
            .rect;
        let target_rect_before = target_info.rect;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            target_info.inner_rect.x,
            target_info.inner_rect.y,
        ));

        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(source));
        let menu = app.state.context_menu.as_mut().expect("pane context menu");
        assert!(matches!(
            menu.kind,
            ContextMenuKind::Pane {
                pane_id,
                source_pane_id: Some(source_pane_id),
                ..
            } if pane_id == target && source_pane_id == source
        ));
        let swap_idx = menu
            .items()
            .iter()
            .position(|item| *item == "Swap with focused pane")
            .expect("swap item");
        menu.list.highlighted = swap_idx;

        handle_context_menu_key(
            &mut app.state,
            &mut app.terminal_runtimes,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 100, 20));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(source));
        assert_eq!(
            app.state
                .view
                .pane_infos
                .iter()
                .find(|info| info.id == source)
                .unwrap()
                .rect,
            target_rect_before
        );
        assert_eq!(
            app.state
                .view
                .pane_infos
                .iter()
                .find(|info| info.id == target)
                .unwrap()
                .rect,
            source_rect_before
        );
    }

    #[tokio::test]
    async fn normal_right_click_keeps_focus_and_exposes_swap_for_reporting_pane() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let source = ws.tabs[0].root_pane;
        let target = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(source);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 100, 20));
        let target_info = app
            .state
            .pane_info_by_id(target)
            .expect("target pane info")
            .clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                target_info.inner_rect.width,
                target_info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        app.state.insert_test_runtime(target, runtime);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            target_info.inner_rect.x,
            target_info.inner_rect.y,
        ));

        assert!(input_rx.try_recv().is_err());
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(source));
        let menu = app.state.context_menu.as_mut().expect("pane context menu");
        assert!(matches!(
            menu.kind,
            ContextMenuKind::Pane {
                pane_id,
                source_pane_id: Some(source_pane_id),
                ..
            } if pane_id == target && source_pane_id == source
        ));
        assert!(menu.items().iter().any(|i| i == "Swap with focused pane"));
    }

    #[tokio::test]
    async fn right_click_passthrough_requires_exact_modifier_match() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let pane_infos = ws.tabs[0].layout.panes(Rect::new(26, 2, 80, 18));
        let info = pane_infos[0].clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        ws.insert_test_runtime(pane_id, runtime);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.view.pane_infos = pane_infos;

        app.state.right_click_passthrough_modifiers = Some(KeyModifiers::CONTROL);

        let col = info.inner_rect.x + 2;
        let row = info.inner_rect.y + 3;
        app.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ..mouse(MouseEventKind::Down(MouseButton::Right), col, row)
        });

        assert_eq!(app.state.mode, Mode::ContextMenu);
        assert!(app.state.context_menu.is_some());
        assert!(app.state.right_click_passthrough.is_none());
        assert!(input_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn right_click_passthrough_does_not_forward_pane_frame_clicks() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let other_pane = ws.test_split(Direction::Vertical);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.right_click_passthrough_modifiers = Some(KeyModifiers::CONTROL);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let info = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)
            .expect("pane info")
            .clone();
        let (runtime, mut input_rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                info.inner_rect.width,
                info.inner_rect.height,
                0,
                b"\x1b[?1002h\x1b[?1006h",
                4,
            );
        app.state.insert_test_runtime(pane_id, runtime);
        app.state.insert_test_runtime(
            other_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(10, 5, b""),
        );

        assert!(app.state.pane_at(info.rect.x, info.rect.y).is_none());
        assert!(app
            .state
            .pane_mouse_target(info.rect.x, info.rect.y)
            .is_some());
        app.handle_mouse(MouseEvent {
            modifiers: KeyModifiers::CONTROL,
            ..mouse(
                MouseEventKind::Down(MouseButton::Right),
                info.rect.x,
                info.rect.y,
            )
        });

        assert_eq!(app.state.mode, Mode::ContextMenu);
        assert!(app.state.context_menu.is_some());
        assert!(app.state.right_click_passthrough.is_none());
        assert!(input_rx.try_recv().is_err());
    }

    fn sample_worktree_open_state() -> crate::app::state::WorktreeOpenState {
        crate::app::state::WorktreeOpenState {
            source_workspace_id: "source".into(),
            source_existing_membership: None,
            source_checkout_path: "/repo/herdr".into(),
            source_repo_root: "/repo/herdr".into(),
            repo_key: "repo-key".into(),
            repo_name: "herdr".into(),
            entries: vec![
                crate::app::state::WorktreeOpenEntry {
                    path: "/repo/herdr".into(),
                    branch: Some("main".into()),
                    is_linked_worktree: false,
                    already_open_ws_idx: Some(0),
                },
                crate::app::state::WorktreeOpenEntry {
                    path: "/repo/herdr-issue".into(),
                    branch: Some("worktree/issue".into()),
                    is_linked_worktree: true,
                    already_open_ws_idx: None,
                },
            ],
            selected: 0,
            query: String::new(),
            search_focused: false,
            error: None,
        }
    }

    #[test]
    fn hovering_context_menu_updates_highlight() {
        let mut app = app_for_mouse_test();
        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        app.state.context_menu = Some(ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 2,
            y: 2,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        });
        app.state.mode = Mode::ContextMenu;

        let menu = app.state.context_menu_rect().unwrap();
        app.handle_mouse(mouse(MouseEventKind::Moved, menu.x + 2, menu.y + 2));

        assert_eq!(app.state.context_menu.unwrap().list.highlighted, 1);
    }

    #[test]
    fn clicking_agent_toast_focuses_target_pane() {
        let mut app = app_for_mouse_test();
        let active = Workspace::test_new("active");
        let mut background = Workspace::test_new("background");
        let first_pane = background.tabs[0].root_pane;
        let target_pane = background.test_split(Direction::Horizontal);
        background.tabs[0].layout.focus_pane(first_pane);

        app.state.workspaces = vec![active, background];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
        app.state.toast_config.delay_seconds = 0;
        let target_terminal_id = app.state.workspaces[1]
            .panes
            .get(&target_pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&target_terminal_id)
            .unwrap()
            .state = AgentState::Working;

        app.state
            .handle_app_event(crate::events::AppEvent::StateChanged {
                pane_id: target_pane,
                agent: Some(Agent::Pi),
                state: AgentState::Idle,
                visible_blocker: false,
                visible_working: false,
                process_exited: false,
                observed_at: std::time::Instant::now(),
            });
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let hit = app.state.view.toast_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            hit.x + 1,
            hit.y + 1,
        ));

        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.workspaces[1].focused_pane_id(), Some(target_pane));
        assert!(app.state.toast.is_none());
        assert_eq!(app.state.mode, Mode::Terminal);

        app.state.last_pane();

        assert_eq!(app.state.active, Some(0));
        assert_eq!(
            app.state.workspaces[0].focused_pane_id(),
            Some(app.state.workspaces[0].tabs[0].root_pane)
        );
    }

    #[test]
    fn toast_click_does_not_steal_mouse_from_settings_overlay() {
        let mut app = app_for_mouse_test();
        let active = Workspace::test_new("active");
        let background = Workspace::test_new("background");
        let target_pane = background.tabs[0].root_pane;
        let workspace_id = background.id.clone();

        app.state.workspaces = vec![active, background];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.toast = Some(crate::app::state::ToastNotification {
            kind: crate::app::state::ToastKind::Finished,
            title: "pi finished".into(),
            context: "background · 2".into(),
            position: None,
            target: Some(crate::app::state::ToastTarget {
                workspace_id,
                pane_id: target_pane,
            }),
        });
        app.state.mode = Mode::Settings;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let hit = app.state.view.toast_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            hit.x + 1,
            hit.y + 1,
        ));

        assert_eq!(app.state.active, Some(0));
        assert!(app.state.toast.is_some());
    }

    #[test]
    fn clicking_confirm_close_accepts_workspace_close() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        app.state.active = Some(0);
        app.state.selected = 1;
        app.state.mode = Mode::ConfirmClose;

        let popup = app.state.confirm_close_rect();
        let inner = Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        );
        let (confirm, _) = crate::ui::confirm_close_button_rects(inner);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            confirm.x,
            confirm.y,
        ));

        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_rename_save_submits_workspace_rename_through_api_path() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("old")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::RenameWorkspace;
        app.state.name_input = "new".into();

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 24));
        let inner = app.state.rename_modal_inner().unwrap();
        let (save, _, _) = crate::ui::rename_button_rects(inner);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            save.x,
            save.y,
        ));

        assert_eq!(app.state.workspaces[0].custom_name.as_deref(), Some("new"));
        assert!(app.event_hub.events_after(0).iter().any(|(_, event)| {
            matches!(event.event, crate::api::schema::EventKind::WorkspaceRenamed)
        }));
    }

    #[test]
    fn clicking_open_worktree_row_selects_and_requests_open() {
        let mut app = app_for_mouse_test();
        app.state.mode = Mode::OpenExistingWorktree;
        app.state.worktree_open = Some(sample_worktree_open_state());
        let inner =
            crate::ui::open_existing_worktree_inner_rect(app.state.screen_rect(), 2).unwrap();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            inner.x + 1,
            inner.y + 5,
        ));

        assert_eq!(app.state.worktree_open.as_ref().unwrap().selected, 1);
        assert!(app.state.request_submit_worktree_open);
    }

    #[test]
    fn clicking_open_worktree_buttons_requests_open_or_cancels() {
        let mut app = app_for_mouse_test();
        app.state.mode = Mode::OpenExistingWorktree;
        app.state.worktree_open = Some(sample_worktree_open_state());
        let inner =
            crate::ui::open_existing_worktree_inner_rect(app.state.screen_rect(), 2).unwrap();
        let (open, _) = crate::ui::open_existing_worktree_button_rects(inner);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            open.x,
            open.y,
        ));

        assert!(app.state.worktree_open.is_some());
        assert!(app.state.request_submit_worktree_open);

        let mut app = app_for_mouse_test();
        app.state.mode = Mode::OpenExistingWorktree;
        app.state.worktree_open = Some(sample_worktree_open_state());
        let inner =
            crate::ui::open_existing_worktree_inner_rect(app.state.screen_rect(), 2).unwrap();
        let (_, cancel) = crate::ui::open_existing_worktree_button_rects(inner);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            cancel.x,
            cancel.y,
        ));

        assert!(app.state.worktree_open.is_none());
        assert_eq!(app.state.mode, Mode::Navigate);
    }

    #[test]
    fn scrolling_open_worktree_picker_moves_selection() {
        let mut app = app_for_mouse_test();
        app.state.mode = Mode::OpenExistingWorktree;
        app.state.worktree_open = Some(sample_worktree_open_state());

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, 1, 1));
        assert_eq!(app.state.worktree_open.as_ref().unwrap().selected, 1);

        app.handle_mouse(mouse(MouseEventKind::ScrollUp, 1, 1));
        assert_eq!(app.state.worktree_open.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn clicking_remove_worktree_buttons_requests_remove_or_cancels() {
        let mut app = app_for_mouse_test();
        app.state.mode = Mode::ConfirmRemoveWorktree;
        app.state.worktree_remove = Some(crate::app::state::WorktreeRemoveState {
            workspace_id: "issue".into(),
            repo_root: "/repo/herdr".into(),
            path: "/repo/herdr-issue".into(),
            error: None,
            removing: false,
            force_confirmation: false,
            branch: None,
        });
        let popup = crate::ui::remove_worktree_popup_rect(app.state.screen_rect()).unwrap();
        let inner = Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        );
        let (_, remove, _) = crate::ui::remove_worktree_button_rects(inner, false, false);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            remove.x,
            remove.y,
        ));

        assert!(app.state.worktree_remove.is_some());
        assert!(app.state.request_submit_worktree_remove);

        let mut app = app_for_mouse_test();
        app.state.mode = Mode::ConfirmRemoveWorktree;
        app.state.worktree_remove = Some(crate::app::state::WorktreeRemoveState {
            workspace_id: "issue".into(),
            repo_root: "/repo/herdr".into(),
            path: "/repo/herdr-issue".into(),
            error: None,
            removing: false,
            force_confirmation: false,
            branch: None,
        });
        let popup = crate::ui::remove_worktree_popup_rect(app.state.screen_rect()).unwrap();
        let inner = Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        );
        let (_, _, cancel) = crate::ui::remove_worktree_button_rects(inner, false, false);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            cancel.x,
            cancel.y,
        ));

        assert!(app.state.worktree_remove.is_none());
        assert_eq!(app.state.mode, Mode::Navigate);
    }

    #[test]
    fn clicking_confirm_close_accepts_after_workspace_context_menu_close() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        let kind = ContextMenuKind::Workspace {
            ws_idx: 1,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &Default::default(),
        );
        let close_idx = items.iter().position(|i| i == "Close").expect("close item");
        app.state.context_menu = Some(ContextMenuState {
            items,
            kind,
            x: 2,
            y: 2,
            list: MenuListState::new(close_idx),
            bora_commands: vec![],
            bora_port: None,
        });
        app.state.mode = Mode::ContextMenu;
        handle_context_menu_key(
            &mut app.state,
            &mut app.terminal_runtimes,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        assert_eq!(app.state.mode, Mode::ConfirmClose);
        assert_eq!(app.state.selected, 1);

        let popup = app.state.confirm_close_rect();
        let inner = Rect::new(
            popup.x + 1,
            popup.y + 1,
            popup.width.saturating_sub(2),
            popup.height.saturating_sub(2),
        );
        let (confirm, _) = crate::ui::confirm_close_button_rects(inner);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            confirm.x + 1,
            confirm.y,
        ));

        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "a");
    }

    #[test]
    fn clicking_context_menu_close_routes_through_api_path() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.confirm_close = false;
        let kind = ContextMenuKind::Workspace {
            ws_idx: 1,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &Default::default(),
        );
        let close_idx = items.iter().position(|i| i == "Close").unwrap() as u16;
        app.state.context_menu = Some(ContextMenuState {
            items,
            kind,
            x: 2,
            y: 2,
            list: MenuListState::new(close_idx as usize),
            bora_commands: vec![],
            bora_port: None,
        });
        app.state.mode = Mode::ContextMenu;

        let menu = app.state.context_menu_rect().unwrap();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 1 + close_idx,
        ));

        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "a");
        assert!(app.event_hub.events_after(0).iter().any(|(_, event)| {
            matches!(event.event, crate::api::schema::EventKind::WorkspaceClosed)
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn keyboard_context_menu_split_keeps_new_runtime() {
        let mut app = app_for_mouse_test();
        app.state.default_shell = "/usr/bin/true".into();
        let (workspace, terminal, runtime) = Workspace::new(
            std::env::current_dir().unwrap_or_else(|_| "/".into()),
            24,
            80,
            app.state.pane_scrollback_limit_bytes,
            app.state.host_terminal_theme,
            app.state.host_terminal_appearance,
            crate::pane::PaneShellConfig::new(&app.state.default_shell, app.state.shell_mode),
            app.event_tx.clone(),
            app.render_notify.clone(),
            app.render_dirty.clone(),
        )
        .expect("workspace should spawn");
        app.state.workspaces = vec![workspace];
        app.terminal_runtimes.insert(terminal.id.clone(), runtime);
        app.state.terminals.insert(terminal.id.clone(), terminal);
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let runtime_count = app.terminal_runtimes.len();
        let kind = ContextMenuKind::Pane {
            ws_idx: 0,
            tab_idx: 0,
            pane_id,
            source_pane_id: None,
            has_manual_label: false,
            right_click_passthrough: false,
        };
        app.state.context_menu = Some(ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 2,
            y: 2,
            list: MenuListState::new(1),
            bora_commands: vec![],
            bora_port: None,
        });
        app.state.mode = Mode::ContextMenu;

        handle_context_menu_key(
            &mut app.state,
            &mut app.terminal_runtimes,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 2);
        assert_eq!(app.terminal_runtimes.len(), runtime_count + 1);

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
    }

    #[test]
    fn dragging_pane_split_updates_captured_layout_ratio() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app.state.view.split_borders[0].clone();
        let before = capture_snapshot(&app.state);
        let drag_row = border.area.y.saturating_add(1);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            border.pos,
            drag_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            border.pos.saturating_add(6),
            drag_row,
        ));

        let after = capture_snapshot(&app.state);
        assert_ne!(root_layout_ratio(&before), root_layout_ratio(&after));
    }

    #[test]
    fn pane_split_hitbox_does_not_overlap_right_pane_content() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_gaps = false;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app.state.view.split_borders[0].clone();
        let row = border.area.y.saturating_add(1);

        assert!(app
            .state
            .find_border_at(border.pos.saturating_sub(1), row)
            .is_none());
        assert!(app.state.find_border_at(border.pos, row).is_some());
        assert!(app
            .state
            .find_border_at(border.pos.saturating_add(1), row)
            .is_none());
    }

    #[test]
    fn pane_split_hitbox_does_not_overlap_bottom_pane_content() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_gaps = false;
        app.state.workspaces[0].test_split(Direction::Vertical);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app.state.view.split_borders[0].clone();
        let col = border.area.x.saturating_add(1);

        assert!(app
            .state
            .find_border_at(col, border.pos.saturating_sub(1))
            .is_none());
        assert!(app.state.find_border_at(col, border.pos).is_some());
        assert!(app
            .state
            .find_border_at(col, border.pos.saturating_add(1))
            .is_none());
    }

    #[test]
    fn borderless_no_gap_split_has_no_mouse_hitbox_over_content() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_borders = false;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app.state.view.split_borders[0].clone();
        let row = border.area.y.saturating_add(1);

        assert!(app.state.find_border_at(border.pos, row).is_none());
    }

    #[test]
    fn bordered_pane_gaps_keep_both_split_borders_draggable() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_gaps = true;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app.state.view.split_borders[0].clone();
        let row = border.area.y.saturating_add(1);

        assert!(app
            .state
            .find_border_at(border.pos.saturating_sub(1), row)
            .is_some());
        assert!(app.state.find_border_at(border.pos, row).is_some());
        assert!(app
            .state
            .find_border_at(border.pos.saturating_add(1), row)
            .is_none());
    }

    #[test]
    fn borderless_pane_gap_is_not_a_pane_but_remains_split_draggable() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_borders = false;
        app.state.pane_gaps = true;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app.state.view.split_borders[0].clone();
        let row = border.area.y.saturating_add(1);
        let gap_col = border.pos.saturating_sub(1);

        assert!(app.state.pane_at(gap_col, row).is_none());
        assert!(app.state.find_border_at(gap_col, row).is_some());
        assert!(app.state.find_border_at(border.pos, row).is_none());
    }

    #[test]
    fn borderless_gap_hitbox_is_empty_when_first_split_side_has_one_cell() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_borders = false;
        app.state.pane_gaps = true;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 2, 4));
        let border = app.state.view.split_borders[0].clone();
        let row = border.area.y.saturating_add(1);
        let candidate_gap_col = border.pos.saturating_sub(1);

        assert!(app.state.pane_frame_at(candidate_gap_col, row).is_some());
        assert!(app.state.find_border_at(candidate_gap_col, row).is_none());
    }

    #[test]
    fn borderless_gap_hitbox_is_empty_when_first_split_side_has_zero_width() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.pane_borders = false;
        app.state.pane_gaps = true;
        app.state.workspaces[0].test_split(Direction::Horizontal);
        app.state.workspaces[0].tabs[0]
            .layout
            .set_ratio_at(&[], 0.1);
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 1, 4));
        let border = app.state.view.split_borders[0].clone();
        let row = border.area.y.saturating_add(1);

        assert_eq!(border.pos, 0);
        assert!(app.state.find_border_at(0, row).is_none());
    }

    #[test]
    fn selecting_from_right_pane_first_content_column_starts_selection() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let second_pane = ws.test_split(Direction::Horizontal);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let second_info = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == second_pane)
            .expect("second pane info")
            .clone();
        let col = second_info.inner_rect.x;
        let row = second_info.inner_rect.y;

        assert!(app.state.find_border_at(col, row).is_none());
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col, row));

        assert!(app.state.drag.is_none());
        assert_eq!(
            app.state
                .selection
                .as_ref()
                .map(|selection| selection.pane_id),
            Some(second_pane)
        );
    }

    #[test]
    fn selecting_from_bottom_pane_first_content_row_starts_selection() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let second_pane = ws.test_split(Direction::Vertical);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let second_info = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == second_pane)
            .expect("second pane info")
            .clone();
        let col = second_info.inner_rect.x;
        let row = second_info.inner_rect.y;

        assert!(app.state.find_border_at(col, row).is_none());
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), col, row));

        assert!(app.state.drag.is_none());
        assert_eq!(
            app.state
                .selection
                .as_ref()
                .map(|selection| selection.pane_id),
            Some(second_pane)
        );
    }

    #[tokio::test]
    async fn dragging_vertical_pane_split_still_resizes_when_pane_mouse_reporting_is_enabled() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let first_pane = ws.tabs[0].root_pane;
        let second_pane = ws.test_split(Direction::Vertical);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let pane_infos = app.state.view.pane_infos.clone();
        let first_info = pane_infos
            .iter()
            .find(|info| info.id == first_pane)
            .expect("first pane info")
            .clone();
        let second_info = pane_infos
            .iter()
            .find(|info| info.id == second_pane)
            .expect("second pane info")
            .clone();

        app.state.insert_test_runtime(
            first_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                first_info.inner_rect.width.max(1),
                first_info.inner_rect.height.max(1),
                b"\x1b[?1002h",
            ),
        );
        app.state.insert_test_runtime(
            second_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                second_info.inner_rect.width.max(1),
                second_info.inner_rect.height.max(1),
                b"\x1b[?1002h",
            ),
        );

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app
            .state
            .view
            .split_borders
            .iter()
            .find(|border| border.direction == Direction::Vertical)
            .expect("vertical split border")
            .clone();
        let before = capture_snapshot(&app.state);
        let drag_col = border.area.x.saturating_add(1);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            drag_col,
            border.pos,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            drag_col,
            border.pos.saturating_add(4),
        ));

        let after = capture_snapshot(&app.state);
        assert_ne!(root_layout_ratio(&before), root_layout_ratio(&after));
    }

    #[tokio::test]
    async fn dragging_horizontal_pane_split_still_resizes_when_pane_mouse_reporting_is_enabled() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("test");
        let first_pane = ws.tabs[0].root_pane;
        let second_pane = ws.test_split(Direction::Horizontal);

        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let pane_infos = app.state.view.pane_infos.clone();
        let first_info = pane_infos
            .iter()
            .find(|info| info.id == first_pane)
            .expect("first pane info")
            .clone();
        let second_info = pane_infos
            .iter()
            .find(|info| info.id == second_pane)
            .expect("second pane info")
            .clone();

        app.state.insert_test_runtime(
            first_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                first_info.inner_rect.width.max(1),
                first_info.inner_rect.height.max(1),
                b"\x1b[?1002h",
            ),
        );
        app.state.insert_test_runtime(
            second_pane,
            crate::terminal::TerminalRuntime::test_with_screen_bytes(
                second_info.inner_rect.width.max(1),
                second_info.inner_rect.height.max(1),
                b"\x1b[?1002h",
            ),
        );

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let border = app
            .state
            .view
            .split_borders
            .iter()
            .find(|border| border.direction == Direction::Horizontal)
            .expect("horizontal split border")
            .clone();
        let before = capture_snapshot(&app.state);
        let drag_row = border.area.y.saturating_add(1);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            border.pos,
            drag_row,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            border.pos.saturating_add(6),
            drag_row,
        ));

        let after = capture_snapshot(&app.state);
        assert_ne!(root_layout_ratio(&before), root_layout_ratio(&after));
    }

    #[test]
    fn wheel_routing_prefers_mouse_reporting() {
        let input_state = crate::pane::InputState {
            alternate_screen: true,
            application_cursor: false,
            bracketed_paste: false,
            focus_reporting: false,
            mouse_protocol_mode: crate::input::MouseProtocolMode::ButtonMotion,
            mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Sgr,
            mouse_alternate_scroll: true,
            modify_other_keys: false,
            color_scheme_reporting: false,
        };

        assert_eq!(wheel_routing(input_state), WheelRouting::MouseReport);
    }

    #[test]
    fn wheel_over_tab_bar_switches_tabs() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("two"));
        ws.test_add_tab(Some("three"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let tab_bar = app.state.view.tab_bar_rect;

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, tab_bar.x + 1, tab_bar.y));
        assert_eq!(app.state.workspaces[0].active_tab, 1);

        app.handle_mouse(mouse(MouseEventKind::ScrollUp, tab_bar.x + 1, tab_bar.y));
        assert_eq!(app.state.workspaces[0].active_tab, 0);

        app.handle_mouse(mouse(MouseEventKind::ScrollUp, tab_bar.x + 1, tab_bar.y));
        assert_eq!(app.state.workspaces[0].active_tab, 2);

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            tab_bar.x + tab_bar.width.saturating_sub(1),
            tab_bar.y,
        ));
        assert_eq!(app.state.workspaces[0].active_tab, 0);
    }

    #[test]
    fn bottom_mode_bar_consumes_hidden_tab_mouse_actions() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("two"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Prefix;
        app.state.tab_bar_position = crate::config::TabBarPositionConfig::Bottom;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let second_tab = app.state.view.tab_hit_areas[1];
        let new_tab = app.state.view.new_tab_hit_area;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            second_tab.x,
            second_tab.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            second_tab.x,
            second_tab.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            second_tab.x,
            second_tab.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            second_tab.x,
            second_tab.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            new_tab.x,
            new_tab.y,
        ));

        app.state.drag = Some(DragState {
            target: DragTarget::SidebarDivider,
        });
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            second_tab.x,
            second_tab.y,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, 0);
        assert_eq!(app.state.workspaces[0].tabs.len(), 2);
        assert!(app.state.context_menu.is_none());
        assert!(app.state.tab_presses.is_empty());
        assert!(app.state.drag.is_none());
    }

    #[test]
    fn right_click_inactive_tab_opens_menu_without_switching_tabs() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("two"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let second_tab = app.state.view.tab_hit_areas[1];

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            second_tab.x + 1,
            second_tab.y,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, 0);
        let menu = app.state.context_menu.as_ref().expect("tab context menu");
        assert_eq!(
            menu.kind,
            ContextMenuKind::Tab {
                ws_idx: 0,
                tab_idx: 1
            }
        );
        assert_eq!(app.state.mode, Mode::ContextMenu);
    }

    #[test]
    fn clicking_tab_context_menu_close_leaves_context_menu_mode() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("two"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let second_tab = app.state.view.tab_hit_areas[1];

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            second_tab.x + 1,
            second_tab.y,
        ));

        let menu = app
            .state
            .context_menu_rect()
            .expect("tab context menu rect");
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 3,
        ));

        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "one");
        assert!(app.state.context_menu.is_none());
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app
            .event_hub
            .events_after(0)
            .iter()
            .any(|(_, event)| { matches!(event.event, crate::api::schema::EventKind::TabClosed) }));
    }

    #[test]
    fn clicking_pane_context_menu_close_leaves_context_menu_mode() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        let first_pane = ws.tabs[0].root_pane;
        let second_pane = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(second_pane);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let first_info = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == first_pane)
            .expect("first pane info")
            .clone();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            first_info.inner_rect.x + 1,
            first_info.inner_rect.y + 1,
        ));

        let menu_state = app.state.context_menu.as_ref().expect("pane context menu");
        let close_idx = menu_state
            .items()
            .iter()
            .position(|item| *item == "Close pane")
            .expect("close pane menu item");
        let menu = app
            .state
            .context_menu_rect()
            .expect("pane context menu rect");
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 1 + close_idx as u16,
        ));

        assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 1);
        assert!(app.state.context_menu.is_none());
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.event_hub.events_after(0).iter().any(|(_, event)| {
            matches!(event.event, crate::api::schema::EventKind::PaneClosed)
        }));
    }

    #[test]
    fn clicking_pane_context_menu_close_last_pane_of_parent_closes_only_it() {
        let mut app = app_for_mouse_test();
        let mut parent = Workspace::test_new("main");
        let pane_id = parent.tabs[0].root_pane;
        mark_worktree_space_member(&mut parent, 0, "repo-key");
        let mut child = Workspace::test_new("issue");
        mark_worktree_space_member(&mut child, 1, "repo-key");
        app.state.workspaces = vec![parent, child];
        app.state.active = Some(0);
        app.state.selected = 1;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let pane_info = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == pane_id)
            .expect("pane info")
            .clone();

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            pane_info.inner_rect.x + 1,
            pane_info.inner_rect.y + 1,
        ));

        let menu_state = app.state.context_menu.as_ref().expect("pane context menu");
        let close_idx = menu_state
            .items()
            .iter()
            .position(|item| *item == "Close pane")
            .expect("close pane menu item");
        let menu = app
            .state
            .context_menu_rect()
            .expect("pane context menu rect");
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            menu.x + 2,
            menu.y + 1 + close_idx as u16,
        ));

        assert_eq!(app.state.selected, 0);
        assert_ne!(app.state.mode, Mode::ConfirmClose);
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "issue");
        assert!(app.state.context_menu.is_none());
    }

    #[test]
    fn wheel_over_overflowing_tab_bar_switches_tabs() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.tabs[0].set_custom_name("very-long-one".into());
        ws.test_add_tab(Some("very-long-two"));
        ws.test_add_tab(Some("very-long-three"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 65, 20));
        assert!(app.state.view.tab_scroll_right_hit_area.width > 0);
        let tab_bar = app.state.view.tab_bar_rect;

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            tab_bar.x + tab_bar.width.saturating_sub(2),
            tab_bar.y,
        ));
        assert_eq!(app.state.workspaces[0].active_tab, 1);

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            tab_bar.x + tab_bar.width.saturating_sub(2),
            tab_bar.y,
        ));
        assert_eq!(app.state.workspaces[0].active_tab, 2);
    }

    #[test]
    fn wheel_outside_tab_bar_does_not_switch_tabs() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("two"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let terminal = app.state.view.terminal_area;

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            terminal.x + 1,
            terminal.y + 1,
        ));

        assert_eq!(app.state.workspaces[0].active_tab, 0);
    }

    #[test]
    fn mobile_switch_button_opens_switcher_and_workspace_row_switches_workspace() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        assert_eq!(app.state.view.layout, ViewLayout::Mobile);

        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));

        assert_eq!(app.state.mode, Mode::Navigate);

        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            viewport.x + 2,
            viewport.y + 4,
        ));

        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn mobile_workspace_panel_scroll_reaches_extra_workspaces() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = (0..12)
            .map(|idx| Workspace::test_new(&format!("ws-{idx}")))
            .collect();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));
        assert_eq!(app.state.mode, Mode::Navigate);

        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;
        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            viewport.x + 2,
            viewport.y,
        ));
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        assert_eq!(app.state.mobile_switcher_scroll, 2);

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            viewport.x + 2,
            viewport.y + 2,
        ));

        assert_eq!(app.state.active, Some(1));
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn mobile_global_scroll_reaches_tabs_and_switches_tab() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("two"));
        ws.test_add_tab(Some("three"));
        ws.test_add_tab(Some("four"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 12));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));

        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;

        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            viewport.x + 2,
            viewport.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::ScrollDown,
            viewport.x + 2,
            viewport.y,
        ));
        assert_eq!(app.state.mobile_switcher_scroll, 4);
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            viewport.x + 2,
            viewport.y + 4,
        ));
        assert_eq!(app.state.workspaces[0].active_tab, 2);
    }

    #[test]
    fn mobile_switcher_new_workspace_opens_prompt_when_enabled() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.prompt_new_workspace_name = true;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));
        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            viewport.x + 2,
            viewport.y + 1,
        ));

        assert_eq!(app.state.mode, Mode::RenameWorkspace);
        assert!(app.state.pending_workspace_create_cwd.is_some());
        assert!(app.state.name_input_replace_on_type);
        assert_eq!(app.state.workspaces.len(), 1);
    }

    #[test]
    fn desktop_new_workspace_opens_prompt_when_enabled() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.prompt_new_workspace_name = true;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 40));
        let new_workspace = app.state.sidebar_new_button_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            new_workspace.x + 1,
            new_workspace.y,
        ));

        assert_eq!(app.state.mode, Mode::RenameWorkspace);
        assert!(app.state.pending_workspace_create_cwd.is_some());
        assert!(app.state.name_input_replace_on_type);
        assert_eq!(app.state.workspaces.len(), 1);
    }

    #[tokio::test]
    async fn desktop_new_workspace_creates_immediately_by_default() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 40));
        let new_workspace = app.state.sidebar_new_button_rect();
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            new_workspace.x + 1,
            new_workspace.y,
        ));

        assert_eq!(app.state.workspaces.len(), 2);
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.pending_workspace_create_cwd.is_none());
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    #[test]
    fn mobile_switcher_new_tab_opens_dialog_when_enabled() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("logs"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));
        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            viewport.x + 2,
            viewport.y + 5,
        ));

        assert_eq!(app.state.mode, Mode::RenameTab);
        assert!(app.state.creating_new_tab);
    }

    #[test]
    fn mobile_switcher_new_tab_skips_dialog_when_prompt_disabled() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.test_add_tab(Some("logs"));
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.prompt_new_tab_name = false;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));
        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;

        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            viewport.x + 2,
            viewport.y + 5,
        ));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(!app.state.creating_new_tab);
        assert!(app.state.request_new_tab);
        assert!(app.state.requested_new_tab_name.is_none());
    }

    #[test]
    fn desktop_new_tab_button_skips_dialog_when_prompt_disabled() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app.state.prompt_new_tab_name = false;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 40));
        let new_tab_area = app.state.view.new_tab_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            new_tab_area.x + 1,
            new_tab_area.y,
        ));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(!app.state.creating_new_tab);
        assert!(app.state.request_new_tab);
        assert!(app.state.requested_new_tab_name.is_none());
    }

    #[test]
    fn mobile_switcher_swallows_non_left_mouse_events() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));
        assert_eq!(app.state.mode, Mode::Navigate);

        let viewport = crate::ui::mobile_switcher_areas(&app.state).viewport;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Right),
            viewport.x + 2,
            viewport.y + 2,
        ));

        assert_eq!(app.state.mode, Mode::Navigate);
        assert!(app.state.context_menu.is_none());
    }

    #[test]
    fn mobile_switch_button_does_not_bypass_rename_modal() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::RenameTab;
        app.state.creating_new_tab = true;
        app.state.name_input = "new tab".into();

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));

        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(!app.state.creating_new_tab);
        assert!(!app.state.request_new_tab);
    }

    #[test]
    fn mobile_switcher_close_returns_to_terminal() {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("one")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 44, 20));
        let switch = app.state.view.mobile_menu_hit_area;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            switch.x + 1,
            switch.y + 1,
        ));
        assert_eq!(app.state.mode, Mode::Navigate);

        let close = crate::ui::mobile_switcher_areas(&app.state).close;
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            close.x + 1,
            close.y,
        ));

        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn wheel_routing_uses_alternate_scroll_in_fullscreen_without_mouse_reporting() {
        let input_state = crate::pane::InputState {
            alternate_screen: true,
            application_cursor: false,
            bracketed_paste: false,
            focus_reporting: false,
            mouse_protocol_mode: crate::input::MouseProtocolMode::None,
            mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Default,
            mouse_alternate_scroll: true,
            modify_other_keys: false,
            color_scheme_reporting: false,
        };

        assert_eq!(wheel_routing(input_state), WheelRouting::AlternateScroll);
    }

    #[test]
    fn wheel_routing_falls_back_to_host_scrollback() {
        let input_state = crate::pane::InputState {
            alternate_screen: false,
            application_cursor: false,
            bracketed_paste: false,
            focus_reporting: false,
            mouse_protocol_mode: crate::input::MouseProtocolMode::None,
            mouse_protocol_encoding: crate::input::MouseProtocolEncoding::Default,
            mouse_alternate_scroll: true,
            modify_other_keys: false,
            color_scheme_reporting: false,
        };

        assert_eq!(wheel_routing(input_state), WheelRouting::HostScroll);
    }

    /// Characterization helper for the right-panel tab-header tests: an app
    /// with one workspace and an expanded right panel with real geometry.
    fn app_with_expanded_right_panel() -> crate::app::App {
        let mut app = app_for_mouse_test();
        app.state.workspaces = vec![Workspace::test_new("test")];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();
        app.state.right_panel_collapsed = false;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 20));
        assert!(
            app.state.view.right_panel_rect.width > 0,
            "expanded right panel must have geometry"
        );
        app
    }

    /// Columns of the tab header row (rp.y) that hit `tab`, resolved from the
    /// shared `right_panel_tab_hit` helper so the test cannot drift from the
    /// renderer's segment layout.
    fn tab_segment_cols(
        rp: ratatui::layout::Rect,
        tab: crate::app::state::RightPanelTab,
    ) -> Vec<u16> {
        (rp.x..rp.x + rp.width)
            .filter(|&col| crate::ui::right_panel::right_panel_tab_hit(col, rp) == Some(tab))
            .collect()
    }

    // Pins the 4-segment tab header layout (row rp.y): the first and last
    // column of each label segment select its tab, divider and past-end
    // columns leave the active tab unchanged, and every header click is
    // consumed.
    #[tokio::test]
    async fn right_panel_tab_header_click_selects_tab_by_label_segment() {
        use crate::app::state::RightPanelTab;

        let mut app = app_with_expanded_right_panel();
        // Widen the panel so all four tab segments fit with room past the last
        // label, exercising the clipped/past-end paths.
        app.state.right_panel_max_width = 60;
        app.state.right_panel_width = 44;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 20));
        let rp = app.state.view.right_panel_rect;
        let header_row = rp.y;

        let changes_cols = tab_segment_cols(rp, RightPanelTab::Changes);
        let checks_cols = tab_segment_cols(rp, RightPanelTab::Checks);
        let issues_cols = tab_segment_cols(rp, RightPanelTab::Issues);
        let prs_cols = tab_segment_cols(rp, RightPanelTab::PullRequests);

        // Segments are laid out left-to-right starting one column after the
        // panel separator, with one divider column between segments.
        assert_eq!(changes_cols.len(), " Changes ".len());
        assert_eq!(checks_cols.len(), " Checks ".len());
        assert_eq!(issues_cols.len(), " Issues ".len());
        assert_eq!(prs_cols.len(), " PRs ".len());
        assert_eq!(changes_cols[0], rp.x + 1);
        assert_eq!(
            checks_cols[0],
            changes_cols.last().copied().expect("cols") + 2
        );
        assert_eq!(
            issues_cols[0],
            checks_cols.last().copied().expect("cols") + 2
        );
        assert_eq!(prs_cols[0], issues_cols.last().copied().expect("cols") + 2);

        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Changes);

        for (tab, cols) in [
            (RightPanelTab::Checks, &checks_cols),
            (RightPanelTab::Issues, &issues_cols),
            (RightPanelTab::PullRequests, &prs_cols),
            (RightPanelTab::Changes, &changes_cols),
        ] {
            for &col in &[cols[0], cols.last().copied().expect("cols")] {
                let action = app.state.handle_mouse(
                    &mut app.terminal_runtimes,
                    crate::app::LOCAL_INPUT_SOURCE,
                    mouse(MouseEventKind::Down(MouseButton::Left), col, header_row),
                );
                assert!(action.is_none(), "tab header click is consumed");
                assert_eq!(app.state.right_panel_active_tab, tab);
            }
        }

        // Divider column between Changes and Checks: no tab change, consumed.
        let divider_col = changes_cols.last().copied().expect("cols") + 1;
        let action = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                divider_col,
                header_row,
            ),
        );
        assert!(action.is_none(), "tab header click is consumed");
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Changes);

        // Past the last label: no tab change, consumed.
        let past_end = prs_cols.last().copied().expect("cols") + 1;
        assert!(
            past_end < rp.x + rp.width,
            "panel widened enough for a past-end column"
        );
        let action = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                past_end,
                header_row,
            ),
        );
        assert!(action.is_none(), "tab header click is consumed");
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Changes);

        app.state.assert_invariants_for_test();
    }

    // Switching tabs resets the shared right-panel scroll; re-clicking the
    // already-active tab keeps it.
    #[tokio::test]
    async fn right_panel_tab_switch_resets_scroll() {
        use crate::app::state::RightPanelTab;

        let mut app = app_with_expanded_right_panel();
        let rp = app.state.view.right_panel_rect;
        let header_row = rp.y;
        let checks_col = tab_segment_cols(rp, RightPanelTab::Checks)[0];

        app.state.right_panel_scroll = 5;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                checks_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Checks);
        assert_eq!(app.state.right_panel_scroll, 0);

        // Re-clicking the active tab is not a switch: scroll is kept.
        app.state.right_panel_scroll = 3;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                checks_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_scroll, 3);

        app.state.assert_invariants_for_test();
    }

    // Characterization: `right_panel_checks_requested` is set only on a
    // transition INTO Checks; re-clicking Checks or switching back to Changes
    // does not set it.
    #[tokio::test]
    async fn right_panel_checks_request_flag_set_only_when_switching_into_checks() {
        use crate::app::state::RightPanelTab;

        let mut app = app_with_expanded_right_panel();
        let rp = app.state.view.right_panel_rect;
        let header_row = rp.y;
        let checks_col = tab_segment_cols(rp, RightPanelTab::Checks)[0];
        let changes_col = tab_segment_cols(rp, RightPanelTab::Changes)[0];

        assert!(!app.state.right_panel_checks_requested);

        // Changes -> Checks requests a checks fetch.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                checks_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Checks);
        assert!(app.state.right_panel_checks_requested);

        // Re-clicking Checks while already on Checks does not re-request.
        app.state.right_panel_checks_requested = false;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                checks_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Checks);
        assert!(!app.state.right_panel_checks_requested);

        // Switching back to Changes never requests checks.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                changes_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Changes);
        assert!(!app.state.right_panel_checks_requested);

        // A fresh Changes -> Checks transition requests again.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                checks_col,
                header_row,
            ),
        );
        assert!(app.state.right_panel_checks_requested);

        app.state.assert_invariants_for_test();
    }

    // Mirror of the checks-flag test: `right_panel_issues_requested` is set
    // only on a transition INTO Issues.
    #[tokio::test]
    async fn right_panel_issues_request_flag_set_only_when_switching_into_issues() {
        use crate::app::state::RightPanelTab;

        let mut app = app_with_expanded_right_panel();
        let rp = app.state.view.right_panel_rect;
        let header_row = rp.y;
        let issues_col = tab_segment_cols(rp, RightPanelTab::Issues)[0];
        let changes_col = tab_segment_cols(rp, RightPanelTab::Changes)[0];

        assert!(!app.state.right_panel_issues_requested);

        // Changes -> Issues requests an issues fetch.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                issues_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Issues);
        assert!(app.state.right_panel_issues_requested);

        // Re-clicking Issues while already on Issues does not re-request.
        app.state.right_panel_issues_requested = false;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                issues_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Issues);
        assert!(!app.state.right_panel_issues_requested);

        // Switching back to Changes never requests issues.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                changes_col,
                header_row,
            ),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Changes);
        assert!(!app.state.right_panel_issues_requested);

        app.state.assert_invariants_for_test();
    }

    /// App with an expanded right panel whose active workspace belongs to a
    /// repo with a populated issues cache.
    fn app_with_issues_cache() -> (crate::app::App, String) {
        let mut app = app_with_expanded_right_panel();
        let identity = "github.com/owner/proj".to_string();
        app.state.workspaces[0].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: "key-p".into(),
            repo_identity: identity.clone(),
            checkout_key: "/repo/proj".into(),
            repo_name: "proj".into(),
            repo_root: std::path::PathBuf::from("/repo/proj"),
            is_linked_worktree: false,
        });
        app.state.repo_issues.insert(
            identity.clone(),
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
        (app, identity)
    }

    /// App with an expanded right panel whose active workspace belongs to a
    /// repo with a populated open-PRs cache, active tab set to PullRequests.
    fn app_with_prs_cache() -> (crate::app::App, String) {
        let mut app = app_with_expanded_right_panel();
        let identity = "github.com/owner/proj".to_string();
        app.state.workspaces[0].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: "key-p".into(),
            repo_identity: identity.clone(),
            checkout_key: "/repo/proj".into(),
            repo_name: "proj".into(),
            repo_root: std::path::PathBuf::from("/repo/proj"),
            is_linked_worktree: false,
        });
        app.state.repo_open_prs.insert(
            identity.clone(),
            crate::workspace::RepoOpenPrs {
                prs: vec![
                    crate::workspace::OpenPr {
                        number: 7,
                        title: "fix: first".into(),
                        url: "https://github.com/owner/proj/pull/7".into(),
                        head_ref_name: "fix/first".into(),
                        is_draft: false,
                        mergeable: Some("MERGEABLE".into()),
                        checks: None,
                    },
                    crate::workspace::OpenPr {
                        number: 12,
                        title: "feat: second".into(),
                        url: "https://github.com/owner/proj/pull/12".into(),
                        head_ref_name: "feat/second".into(),
                        is_draft: false,
                        mergeable: None,
                        checks: None,
                    },
                ],
                error: None,
            },
        );
        app.state.right_panel_active_tab = crate::app::state::RightPanelTab::PullRequests;
        (app, identity)
    }

    #[tokio::test]
    async fn right_panel_pr_at_row_walks_flat_layout_with_scroll() {
        let (mut app, _identity) = app_with_prs_cache();
        let rp = app.state.view.right_panel_rect;
        let body_start = rp.y + 1;

        assert_eq!(
            app.state.right_panel_pr_at_row(body_start),
            Some((
                7,
                "https://github.com/owner/proj/pull/7".to_string(),
                "fix/first".to_string()
            ))
        );
        assert_eq!(
            app.state.right_panel_pr_at_row(body_start + 1),
            Some((
                12,
                "https://github.com/owner/proj/pull/12".to_string(),
                "feat/second".to_string()
            ))
        );
        // Header row and rows past the list resolve to nothing.
        assert_eq!(app.state.right_panel_pr_at_row(rp.y), None);
        assert_eq!(app.state.right_panel_pr_at_row(body_start + 2), None);

        // Nonzero scroll shifts the flat index.
        app.state.right_panel_scroll = 1;
        assert_eq!(
            app.state.right_panel_pr_at_row(body_start),
            Some((
                12,
                "https://github.com/owner/proj/pull/12".to_string(),
                "feat/second".to_string()
            ))
        );

        // An errored cache never resolves rows.
        app.state.right_panel_scroll = 0;
        app.state
            .repo_open_prs
            .get_mut(&_identity)
            .expect("cache")
            .error = Some("gh failed".into());
        assert_eq!(app.state.right_panel_pr_at_row(body_start), None);
    }

    #[tokio::test]
    async fn right_panel_pr_row_click_opens_repo_pr_context_menu() {
        let (mut app, _identity) = app_with_prs_cache();
        let rp = app.state.view.right_panel_rect;
        let body_start = rp.y + 1;

        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                rp.x + 2,
                body_start + 1,
            ),
        );

        assert_eq!(app.state.mode, Mode::ContextMenu);
        let menu = app.state.context_menu.as_ref().expect("context menu open");
        match &menu.kind {
            crate::app::state::ContextMenuKind::RepoPr {
                ws_idx,
                number,
                url,
                head_ref,
            } => {
                assert_eq!(*ws_idx, 0);
                assert_eq!(*number, 12);
                assert_eq!(url, "https://github.com/owner/proj/pull/12");
                assert_eq!(head_ref, "feat/second");
            }
            other => panic!("expected RepoPr context menu, got {other:?}"),
        }

        app.state.assert_invariants_for_test();
    }

    #[tokio::test]
    async fn right_panel_prs_request_flag_set_only_when_switching_into_prs() {
        let mut app = app_with_expanded_right_panel();
        use crate::app::state::RightPanelTab;
        let rp = app.state.view.right_panel_rect;
        let prs_col = tab_segment_cols(rp, RightPanelTab::PullRequests)[0];
        let changes_col = tab_segment_cols(rp, RightPanelTab::Changes)[0];

        assert!(!app.state.right_panel_prs_requested);

        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(MouseEventKind::Down(MouseButton::Left), prs_col, rp.y),
        );
        assert_eq!(
            app.state.right_panel_active_tab,
            RightPanelTab::PullRequests
        );
        assert!(app.state.right_panel_prs_requested);

        // Re-clicking PRs while already on PRs does not re-request.
        app.state.right_panel_prs_requested = false;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(MouseEventKind::Down(MouseButton::Left), prs_col, rp.y),
        );
        assert!(!app.state.right_panel_prs_requested);

        // Switching back to Changes never requests PRs.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(MouseEventKind::Down(MouseButton::Left), changes_col, rp.y),
        );
        assert_eq!(app.state.right_panel_active_tab, RightPanelTab::Changes);
        assert!(!app.state.right_panel_prs_requested);

        app.state.assert_invariants_for_test();
    }

    #[tokio::test]
    async fn right_panel_issue_at_row_walks_flat_layout_with_scroll() {
        let (app, identity) = app_with_issues_cache();
        let rp = app.state.view.right_panel_rect;
        let body_start = rp.y + 1;

        assert_eq!(
            app.state.right_panel_issue_at_row(body_start),
            Some((7, "https://github.com/owner/proj/issues/7".to_string()))
        );
        assert_eq!(
            app.state.right_panel_issue_at_row(body_start + 1),
            Some((12, "https://github.com/owner/proj/issues/12".to_string()))
        );
        // Header row and rows past the list resolve to nothing.
        assert_eq!(app.state.right_panel_issue_at_row(rp.y), None);
        assert_eq!(app.state.right_panel_issue_at_row(body_start + 2), None);

        // Nonzero scroll shifts the flat index.
        let mut app = app;
        app.state.right_panel_scroll = 1;
        assert_eq!(
            app.state.right_panel_issue_at_row(body_start),
            Some((12, "https://github.com/owner/proj/issues/12".to_string()))
        );
        assert_eq!(app.state.right_panel_issue_at_row(body_start + 1), None);

        // An errored cache never resolves rows.
        app.state.right_panel_scroll = 0;
        app.state
            .repo_issues
            .get_mut(&identity)
            .expect("cache")
            .error = Some("gh failed".into());
        assert_eq!(app.state.right_panel_issue_at_row(body_start), None);
    }

    #[tokio::test]
    async fn right_panel_issue_row_click_opens_repo_issue_context_menu() {
        let (mut app, _identity) = app_with_issues_cache();
        app.state.right_panel_active_tab = crate::app::state::RightPanelTab::Issues;
        app.state.right_panel_scroll = 1;
        let rp = app.state.view.right_panel_rect;
        let body_start = rp.y + 1;

        // With scroll 1, the first body row is the second issue.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                rp.x + 2,
                body_start,
            ),
        );

        assert_eq!(app.state.mode, Mode::ContextMenu);
        let menu = app.state.context_menu.as_ref().expect("context menu open");
        match &menu.kind {
            crate::app::state::ContextMenuKind::RepoIssue {
                number,
                url,
                flow_available,
            } => {
                assert_eq!(*number, 12);
                assert_eq!(url, "https://github.com/owner/proj/issues/12");
                assert!(
                    !flow_available,
                    "no flow template configured in this fixture"
                );
            }
            other => panic!("expected RepoIssue context menu, got {other:?}"),
        }
        assert_eq!(menu.items, ["Open in browser", "Copy URL"]);

        app.state.assert_invariants_for_test();
    }

    #[tokio::test]
    async fn right_panel_issue_menu_offers_flow_run_when_template_configured() {
        let (mut app, _identity) = app_with_issues_cache();
        app.state.flow_command_template = Some("bora-flow run {issue}".into());
        app.state.right_panel_active_tab = crate::app::state::RightPanelTab::Issues;
        let rp = app.state.view.right_panel_rect;
        let body_start = rp.y + 1;

        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                rp.x + 2,
                body_start,
            ),
        );

        assert_eq!(app.state.mode, Mode::ContextMenu);
        let menu = app.state.context_menu.as_ref().expect("context menu open");
        assert!(matches!(
            menu.kind,
            crate::app::state::ContextMenuKind::RepoIssue {
                flow_available: true,
                ..
            }
        ));
        assert_eq!(
            menu.items,
            [
                "Run with bora-flow",
                crate::app::state::CONTEXT_MENU_SEPARATOR,
                "Open in browser",
                "Copy URL",
            ]
        );

        app.state.assert_invariants_for_test();
    }

    #[tokio::test]

    async fn sidebar_repo_header_plus_click_requests_create_worktree_modal() {
        let mut app = app_for_mouse_test();
        let identity = "github.com/owner/proj".to_string();
        app.state
            .workspaces
            .push(crate::workspace::Workspace::test_new("proj"));
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();
        app.state.workspaces[0].cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: "key-p".into(),
            repo_identity: identity.clone(),
            checkout_key: "/repo/proj".into(),
            repo_name: "proj".into(),
            repo_root: std::path::PathBuf::from("/repo/proj"),
            is_linked_worktree: false,
        });
        let plus_rect = ratatui::layout::Rect::new(3, 5, 3, 1);
        app.state.view.worktree_new_hit_areas = vec![crate::app::state::WorktreeNewHitArea {
            repo_identity: identity.clone(),
            rect: plus_rect,
        }];

        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                plus_rect.x + 1,
                plus_rect.y,
            ),
        );

        assert_eq!(app.state.request_open_create_worktree, Some(identity));
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn project_view_section_plus_click_requests_section_worktree_create() {
        // T4 (bora-79l, P3): clicking the SectionRow header's trailing
        // 3-cell "+" must reach `start_section_worktree_create` — not the
        // collapse toggle of the Section area underneath. The areas here
        // come from the REAL geometry walk (`compute_workspace_list_areas_
        // all`), so this goes red if any of the wiring drops out: the
        // emission, the before-Section ordering (first-match hit-test), or
        // the click arm. It also pins that the + click does NOT collapse
        // the section and does NOT record a workspace press.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let identity = "github.com/owner/proj".to_string();
        let mut ws = Workspace::test_new("proj");
        ws.cached_git_branch = Some("main".into());
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: "key-p".into(),
            repo_identity: identity.clone(),
            checkout_key: "/repo/proj".into(),
            repo_name: "proj".into(),
            repo_root: std::path::PathBuf::from("/repo/proj"),
            is_linked_worktree: false,
        });
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();

        let area = app.state.view.sidebar_rect;
        let (_cards, _headers, project_rows) =
            crate::ui::compute_workspace_list_areas_all(&app.state, area);
        let plus = project_rows
            .iter()
            .find(|row| {
                matches!(
                    row.target,
                    crate::app::state::ProjectRowTarget::SectionNew { .. }
                )
            })
            .expect("the SectionRow must emit a SectionNew + area");
        let plus_rect = plus.rect;
        assert_eq!(
            plus.target,
            crate::app::state::ProjectRowTarget::SectionNew {
                repo_identity: identity.clone(),
                branch: "main".into(),
            }
        );
        assert!(
            project_rows.iter().any(|row| matches!(
                row.target,
                crate::app::state::ProjectRowTarget::Section { .. }
            )),
            "the full-row Section area is still emitted underneath"
        );

        app.state.view.project_row_areas = project_rows;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                plus_rect.x + 1,
                plus_rect.y,
            ),
        );

        assert_eq!(
            app.state.request_section_worktree_create,
            Some((identity, "main".into()))
        );
        assert!(
            app.state.collapsed_space_keys.is_empty(),
            "the + click must not toggle the section's collapse"
        );
        assert!(
            app.state.workspace_presses.is_empty(),
            "the + click must not record a workspace press (click-only, like OpenPr)"
        );
        app.state.assert_invariants_for_test();
    }

    /// Local git scaffolding for the T4 demo test (worktrees.rs's own
    /// helpers are private to its test module): a committed repo on
    /// `main`.
    fn section_plus_demo_repo(name: &str) -> std::path::PathBuf {
        fn git(repo: &std::path::Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .unwrap();
            assert!(
                status.success(),
                "git -C {} {} failed",
                repo.display(),
                args.join(" ")
            );
        }
        let repo = unique_temp_path(name);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(&repo, &["config", "user.email", "herdr@example.invalid"]);
        git(&repo, &["config", "user.name", "Herdr Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "--quiet", "-m", "initial"]);
        repo
    }
    #[tokio::test]
    async fn section_plus_click_creates_workspace_in_section_with_clickable_block() {
        // T4 demo (bora-79l.12, gate 2) — the whole chain against real git:
        // the real geometry walk emits the SectionNew "+" area; a real
        // mouse click lands in it; App's drain reaches
        // `start_section_worktree_create`; the deferred worktree.create
        // runs `git worktree add` on the worker thread; the finished event
        // opens the workspace; and the new workspace renders in Project
        // view under the same ProjectRow as the clicked section, with a
        // clickable PaneDotsRow block (T1: the block is the card). Fica
        // vermelho em qualquer elo: emissão, clique, drain, criação,
        // abertura, ou o card pós-criação.
        let repo = section_plus_demo_repo("section-plus-demo-repo");
        let worktree_root = unique_temp_path("section-plus-demo-root");
        let identity = "github.com/owner/demo".to_string();

        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        app.state.worktree_directory = worktree_root.clone();
        let mut ws = Workspace::test_new("demo");
        ws.identity_cwd = repo.clone();
        ws.cached_git_branch = Some("main".into());
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: "demo-key".into(),
            repo_identity: identity.clone(),
            checkout_key: repo.display().to_string(),
            repo_name: "demo".into(),
            repo_root: repo.clone(),
            is_linked_worktree: false,
        });
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();
        app.state.view.sidebar_rect = ratatui::layout::Rect::new(0, 0, 40, 20);

        // 1. The real geometry walk emits the "+" for the `main` section.
        let area = app.state.view.sidebar_rect;
        let (_cards, _headers, rows) =
            crate::ui::compute_workspace_list_areas_all(&app.state, area);
        let plus_rect = rows
            .iter()
            .find(|row| {
                matches!(
                    row.target,
                    crate::app::state::ProjectRowTarget::SectionNew { .. }
                )
            })
            .expect("the SectionRow must emit a SectionNew + area")
            .rect;

        // 2. A real click inside those 3 cells.
        app.state.view.project_row_areas = rows;
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                plus_rect.x + 1,
                plus_rect.y,
            ),
        );
        assert_eq!(
            app.state.request_section_worktree_create.take(),
            Some((identity.clone(), "main".into()))
        );

        // 3. App's drain (mod.rs's one-liner — same method, same args).
        app.start_section_worktree_create(&identity, "main");

        // 4. The worker thread ran real git; pump the finished event
        //    through the same handler App::run uses.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let event = loop {
            if let Ok(event) = app.event_rx.try_recv() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the worktree create event"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        match event {
            crate::events::AppEvent::WorktreeAddFinished(result) => {
                app.handle_worktree_add_finished(*result);
            }
            other => panic!("unexpected event: {other:?}"),
        }

        // 5. The workspace exists, focused, on the shared generator's
        //    namespaced branch, under the worktree directory.
        assert_eq!(
            app.state.workspaces.len(),
            2,
            "worktree + workspace created"
        );
        assert_eq!(app.state.active, Some(1), "focus: true switched to it");
        let new_checkout = app.state.workspaces[1].identity_cwd.clone();
        assert!(
            new_checkout.starts_with(&worktree_root),
            "checkout under the configured worktree dir: {new_checkout:?}"
        );
        let branch_out = std::process::Command::new("git")
            .arg("-C")
            .arg(&new_checkout)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert!(branch_out.status.success());
        let new_branch = String::from_utf8(branch_out.stdout)
            .unwrap()
            .trim()
            .to_string();
        assert!(
            new_branch.starts_with("worktree/"),
            "the branch came from generated_branch_slug — the generator the \
             other modes' + use (its `worktree/` namespace): {new_branch:?}"
        );
        // "Na section certa": the new worktree belongs to the clicked
        // section's repo (membership's repo_root is the source checkout),
        // not to whatever workspace happened to be focused.
        assert_eq!(
            app.state.workspaces[1]
                .worktree_space()
                .map(|membership| membership.repo_root.as_path()),
            Some(repo.as_path()),
            "membership records the section's repo root"
        );

        // 6. Post-creation render: the new workspace's block is clickable —
        //    a WorkspaceCardArea spanning its 2-row PaneDotsRow, resolving
        //    through the same workspace_at_row every press/drag/switch uses
        //    — and both sections sit under ONE ProjectRow (the project
        //    context of the clicked +), whose header carries its own +.
        let (cards, _headers2, rows2) =
            crate::ui::compute_workspace_list_areas_all(&app.state, area);
        let card_rect = cards
            .iter()
            .find(|card| card.ws_idx == 1)
            .expect("the new workspace's PaneDotsRow block must carry a card")
            .rect;
        assert_eq!(card_rect.height, 2, "the block spans both rows (T1)");
        app.state.view.workspace_card_areas = cards;
        assert_eq!(
            app.state.workspace_at_row(card_rect.y + 1),
            Some(1),
            "a click on the block resolves to the new workspace"
        );
        assert_eq!(
            rows2
                .iter()
                .filter(|row| {
                    matches!(
                        row.target,
                        crate::app::state::ProjectRowTarget::Project { .. }
                    )
                })
                .count(),
            1,
            "both SectionRows render under the same ProjectRow — the \
             clicked section's project context: {rows2:?}"
        );
        assert!(
            rows2.iter().any(|row| matches!(
                row.target,
                crate::app::state::ProjectRowTarget::SectionNew { .. }
            )),
            "the new section's header carries its own + too"
        );

        for (_, runtime) in app.terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let remove = crate::worktree::build_worktree_remove_command(&repo, &new_checkout, false);
        crate::worktree::run_worktree_command(&remove).unwrap();
        let _ = std::fs::remove_dir_all(worktree_root);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn commands_section_item_click_launches_tagged_command() {
        let mut app = app_for_mouse_test();
        let mut ws = Workspace::test_new("one");
        ws.cached_commands = Some(vec![crate::bora_config::BoraCommand {
            label: "dev".into(),
            command: "htop".into(),
            mode: crate::bora_config::BoraCommandMode::Pane,
            branch: None,
        }]);
        app.state.workspaces = vec![ws];
        app.state.active = Some(0);

        // A COMMANDS row click resolves through the tick-refreshed cache and
        // dispatches the launch into the row's workspace (bora-55c.3).
        assert!(app
            .state
            .handle_project_row_click(crate::app::state::ProjectRowTarget::SectionItem {
                kind: crate::ui::SectionDescriptor::from_wire_name("commands")
                    .expect("registry has a commands descriptor"),
                label: "dev".to_string(),
                ws_idx: Some(0),
            })
            .is_none());
        let launched = app
            .state
            .pending_bora_command
            .take()
            .expect("a COMMANDS row click must dispatch the launch");
        assert_eq!(launched.command, "htop");
        assert_eq!(launched.label.as_deref(), Some("dev"));
        assert_eq!(launched.ws_idx, 0);

        // A non-COMMANDS row (and a label the cache does not declare) never
        // launches.
        assert!(app
            .state
            .handle_project_row_click(crate::app::state::ProjectRowTarget::SectionItem {
                kind: crate::ui::SectionDescriptor::from_wire_name("todos")
                    .expect("registry has a todos descriptor"),
                label: "dev".to_string(),
                ws_idx: None,
            })
            .is_none());
        assert!(app
            .state
            .handle_project_row_click(crate::app::state::ProjectRowTarget::SectionItem {
                kind: crate::ui::SectionDescriptor::from_wire_name("commands")
                    .expect("registry has a commands descriptor"),
                label: "not-declared".to_string(),
                ws_idx: Some(0),
            })
            .is_none());
        assert!(app.state.pending_bora_command.is_none());
    }

    #[test]
    fn open_pr_row_click_requests_the_pr_worktree() {
        // A PR row in the project-level PULL REQUESTS band must reach the same
        // destination as the right panel's right-click "Open in worktree":
        // `request_open_pr_worktree`, which `App` drains into
        // `start_pr_worktree_create`. If the two ever diverged, the sidebar row
        // and the menu item would claim to do the same thing and not.
        let mut app = app_for_mouse_test();
        assert!(app
            .state
            .handle_project_row_click(crate::app::state::ProjectRowTarget::OpenPr {
                ws_idx: 2,
                number: 42,
            })
            .is_none());
        assert_eq!(app.state.request_open_pr_worktree, Some((2, 42)));
    }

    #[test]
    fn section_row_click_toggles_its_own_workspace_collapse_bora_c1h() {
        // bora-c1h G9: a SectionRow click toggles ITS OWN workspace's
        // collapse (`wsec:{ws_idx}`), not the checkout's — a checkout with
        // 2+ open workspaces must collapse them independently.
        let mut app = app_for_mouse_test();
        let target = crate::app::state::ProjectRowTarget::Section {
            ws_idx: 3,
            checkout_key: "/repo/checkout".into(),
            collapse_key: "wsec:3".into(),
        };
        assert!(app.state.handle_project_row_click(target.clone()).is_none());
        assert!(
            app.state.collapsed_space_keys.contains("wsec:3"),
            "first click collapses the section"
        );
        assert!(app.state.handle_project_row_click(target).is_none());
        assert!(
            !app.state.collapsed_space_keys.contains("wsec:3"),
            "second click expands it back"
        );
    }

    #[test]
    fn section_row_right_click_opens_project_member_targets_from_checkout_key() {
        // bora-c1h G9: right-click on a Section row still opens the
        // bora-uqv `ProjectMemberTargets` menu, resolving `member_dir`
        // straight from `checkout_key` — no more `wt:`-prefix stripping.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let row_rect = Rect::new(0, 5, 20, 1);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: row_rect,
            target: ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: "/repo/checkout".into(),
                collapse_key: "wsec:0".into(),
            },
        }];
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(MouseEventKind::Down(MouseButton::Right), 2, 5),
        );
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("right-click on a Section row must open a context menu");
        assert!(
            matches!(
                &menu.kind,
                crate::app::state::ContextMenuKind::ProjectMemberTargets { member_dir }
                    if member_dir == "/repo/checkout"
            ),
            "expected ProjectMemberTargets{{member_dir: \"/repo/checkout\"}}, got {:?}",
            menu.kind
        );
    }

    #[test]
    fn project_view_workspace_press_lives_on_the_pane_dots_block_not_the_branch_line() {
        // P2, bora-79l T1 — attribution flip of
        // `project_view_section_row_press_records_workspace_press_only_for_section_targets`:
        // the `WorkspacePressState` (what drag-reorder and mouse-up's
        // `chrome_press_action`/FocusWorkspace feed on) is recorded from
        // the workspace's own 2-row block via `workspace_at_row`, never
        // from the branch line or a band row. Goes red if the `Section`
        // arm regresses to recording a press, or if the block's card
        // stops feeding one.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let branch_row = Rect::new(0, 5, 20, 1);
        let band_row = Rect::new(0, 8, 20, 1);
        let block = Rect::new(0, 6, 20, 2);
        app.state.view.project_row_areas = vec![
            ProjectRowHitArea {
                rect: branch_row,
                target: ProjectRowTarget::Section {
                    ws_idx: 3,
                    checkout_key: "/repo/checkout".into(),
                    collapse_key: "wsec:3".into(),
                },
            },
            ProjectRowHitArea {
                rect: band_row,
                target: ProjectRowTarget::Band {
                    collapse_key: "band:commands".into(),
                },
            },
        ];
        app.state.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            ws_idx: 3,
            rect: block,
            indented: true,
        }];

        // The branch line records nothing — decision 7.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            1,
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 5),
        );
        assert!(
            !app.state.workspace_presses.contains_key(&1),
            "a branch-line press must not record a WorkspacePressState (P2)"
        );

        // BOTH rows of the block record the press (l1 and l2).
        for row in [block.y, block.y + 1] {
            app.state.handle_mouse(
                &mut app.terminal_runtimes,
                2,
                mouse(MouseEventKind::Down(MouseButton::Left), 2, row),
            );
            let press = app
                .state
                .workspace_presses
                .get(&2)
                .expect("a block press must record a WorkspacePressState");
            assert_eq!(press.ws_idx, 3);
            assert_eq!(press.start_row, row);
        }

        // A band row records nothing, as ever.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            3,
            mouse(MouseEventKind::Down(MouseButton::Left), 2, 8),
        );
        assert!(
            !app.state.workspace_presses.contains_key(&3),
            "a Band row must not record a workspace press"
        );
    }

    #[test]
    fn project_view_pane_dots_block_drag_opens_workspace_reorder_for_linked_worktree() {
        // P2, bora-79l T1 — attribution flip of
        // `project_view_section_row_drag_opens_workspace_reorder_for_linked_worktree`:
        // the drag now STARTS on the workspace's own 2-row block, and a
        // linked worktree must still reorder there (Project view gives
        // every workspace its own card; the Flat/Repo refusal stays
        // pinned by `flat_mode_drag_reorders_linked_worktree`). Goes red
        // if the block stops feeding `workspace_presses`, if the branch
        // line regresses to starting a drag, or if the Project-view
        // exemption in `can_reorder`/`workspace_move_block_params` is
        // dropped.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("linked");
        ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "grp".into(),
            label: "grp".into(),
            repo_root: "/repo".into(),
            checkout_path: "/repo/checkout".into(),
            is_linked_worktree: true,
        });
        app.state.workspaces = vec![ws];
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));
        let list_area = app.state.workspace_list_rect();
        let branch_row = Rect::new(list_area.x, list_area.y, list_area.width, 1);
        let block = Rect::new(list_area.x, list_area.y + 1, list_area.width, 2);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: branch_row,
            target: ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: "/repo/checkout".into(),
                collapse_key: "wsec:0".into(),
            },
        }];
        app.state.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            ws_idx: 0,
            rect: block,
            indented: true,
        }];

        // The branch line starts no drag — decision 7's drag half.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            branch_row.x + 2,
            branch_row.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            branch_row.x + 4,
            branch_row.y,
        ));
        assert!(
            app.state.drag.is_none(),
            "a drag gesture on the branch line must not open WorkspaceReorder (P2)"
        );

        // The same gesture one row down, on the block, opens the reorder.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            block.x + 2,
            block.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            block.x + 4,
            block.y,
        ));

        assert!(
            matches!(
                &app.state.drag,
                Some(DragState {
                    target: DragTarget::WorkspaceReorder {
                        source_ws_idx: 0,
                        ..
                    }
                })
            ),
            "expected an open WorkspaceReorder drag from the linked-worktree's \
             PaneDotsRow block"
        );
    }

    #[test]
    fn project_view_dot_cell_wins_over_the_block_inside_its_own_rect() {
        // P2, bora-79l T1 item 6: inside a dot's own 1-cell rect the dot
        // wins — the dispatcher resolves `project_row_areas` BEFORE
        // `workspace_card_areas`, so a click on a dot focuses THAT pane
        // (FocusPane on press) while the same row's non-dot cells switch
        // workspace (FocusWorkspace on mouse-up). Goes red if the dispatch
        // order flips or the dot hit areas stop landing inside the card's
        // rows.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("two-panes");
        let root_pane = ws.tabs[0].root_pane;
        ws.test_split(Direction::Vertical);
        app.state.workspaces = vec![ws];
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 106, 20));

        let card = app
            .state
            .view
            .workspace_card_areas
            .iter()
            .find(|card| card.ws_idx == 0)
            .expect("the PaneDotsRow block must be the workspace's card");
        let card = *card;
        let mut dot_hits: Vec<_> = app
            .state
            .view
            .project_row_areas
            .iter()
            .filter(|area| matches!(area.target, ProjectRowTarget::Pane { .. }))
            .cloned()
            .collect();
        dot_hits.sort_by_key(|area| area.rect.x);
        assert_eq!(dot_hits.len(), 2, "one dot hit per pane: {dot_hits:?}");
        // The dot rects live INSIDE the card's rows — the overlap is the
        // point: the dot must win by dispatch order, not by geometry.
        for hit in &dot_hits {
            assert!(
                hit.rect.y >= card.rect.y && hit.rect.y < card.rect.y + card.rect.height,
                "the dot rect {:?} must sit inside the block card {:?}",
                hit.rect,
                card.rect
            );
        }

        // Click the second dot's own cell: FocusPane for that pane.
        let action = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                dot_hits[1].rect.x,
                dot_hits[1].rect.y,
            ),
        );
        match action {
            Some(MouseAction::FocusPane { ws_idx, pane_id }) => {
                assert_eq!(ws_idx, 0);
                assert_ne!(
                    pane_id, root_pane,
                    "the second dot must focus the second pane, not the root"
                );
            }
            _ => panic!("expected FocusPane from a dot cell"),
        }

        // One column left of the first dot — still the block's l2 row,
        // no dot rect: the card wins, FocusWorkspace on mouse-up.
        let non_dot_col = dot_hits[0].rect.x.saturating_sub(1);
        let down = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                non_dot_col,
                dot_hits[0].rect.y,
            ),
        );
        assert!(
            down.is_none(),
            "a non-dot block cell records no action on press"
        );
        let up = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                non_dot_col,
                dot_hits[0].rect.y,
            ),
        );
        assert!(
            matches!(up, Some(MouseAction::FocusWorkspace { ws_idx: 0 })),
            "a non-dot cell on the block must switch workspace (FocusWorkspace)"
        );
    }

    #[test]
    fn project_view_pane_dots_block_click_focuses_the_workspace_branch_line_does_not() {
        // P2, bora-79l T1 — attribution flip of
        // `project_view_section_row_click_without_drag_focuses_the_workspace`:
        // a plain click (no drag) on the workspace's own 2-row block must
        // update `active` (the block's card feeds `workspace_presses`;
        // mouse-up's `chrome_press_action` turns it into
        // FocusWorkspace), while the same click on the branch line above
        // must do NOTHING — decision 7. Goes red if either side regresses.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        app.state.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        app.state.active = Some(0);
        app.state.selected = 0;
        let branch_row = Rect::new(0, 5, 20, 1);
        let block = Rect::new(0, 6, 20, 2);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: branch_row,
            target: ProjectRowTarget::Section {
                ws_idx: 1,
                checkout_key: "/repo/checkout-b".into(),
                collapse_key: "wsec:1".into(),
            },
        }];
        app.state.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            ws_idx: 1,
            rect: block,
            indented: true,
        }];

        // Branch line: click does not switch workspace.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            branch_row.x + 5,
            branch_row.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            branch_row.x + 5,
            branch_row.y,
        ));
        assert_eq!(
            app.state.active,
            Some(0),
            "a click on the branch line must NOT switch workspace (P2, decision 7)"
        );

        // The block one row down: the same click focuses the workspace —
        // on BOTH of its rows.
        for row in [block.y, block.y + 1] {
            app.handle_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                block.x + 5,
                row,
            ));
            app.handle_mouse(mouse(
                MouseEventKind::Up(MouseButton::Left),
                block.x + 5,
                row,
            ));
            assert_eq!(
                app.state.active,
                Some(1),
                "a click on the block's row {row} must focus the workspace, \
                 same as a Flat/Repo card"
            );
        }
    }

    #[test]
    fn project_view_section_row_caret_still_collapses_rest_of_row_does_nothing() {
        // P2, bora-79l T1 — attribution flip of
        // `project_view_section_row_caret_collapses_rest_of_row_focuses_without_collapsing`:
        // the caret split survives (only the caret `▾`/`▸` column
        // collapses), but the "rest of the row selects" half moved one row
        // down — clicking the branch line's non-caret region now does
        // NOTHING (the workspace's own block carries selection). Goes red
        // if the caret loses its collapse or the branch line regresses to
        // selecting.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        app.state.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];
        app.state.active = Some(0);
        app.state.selected = 0;
        let row_rect = Rect::new(0, 5, 20, 1);
        let collapse_key = "wsec:1".to_string();
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: row_rect,
            target: ProjectRowTarget::Section {
                ws_idx: 1,
                checkout_key: "/repo/checkout-b".into(),
                collapse_key: collapse_key.clone(),
            },
        }];

        // Click away from the caret column: no focus, no collapse.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            row_rect.x + 5,
            row_rect.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            row_rect.x + 5,
            row_rect.y,
        ));
        assert_eq!(
            app.state.active,
            Some(0),
            "a click away from the caret must NOT switch workspace anymore (P2)"
        );
        assert!(
            !app.state.collapsed_space_keys.contains(&collapse_key),
            "a click away from the caret must not collapse the row"
        );

        // Click on the caret column: collapses.
        app.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            row_rect.x,
            row_rect.y,
        ));
        app.handle_mouse(mouse(
            MouseEventKind::Up(MouseButton::Left),
            row_rect.x,
            row_rect.y,
        ));
        assert!(
            app.state.collapsed_space_keys.contains(&collapse_key),
            "a click on the caret must collapse the row"
        );
    }

    #[test]
    fn project_view_pane_dots_block_right_click_opens_the_full_workspace_menu() {
        // P2, bora-79l T1 — attribution flip of
        // `project_view_section_row_right_click_merges_full_workspace_menu_with_project_membership`:
        // right-click on the workspace's own 2-row block resolves the
        // block's `WorkspaceCardArea` through `workspace_at_row` and gets
        // the FULL menu (`ContextMenuKind::GitWorkspace`/
        // `Workspace` + the project-membership splice). Goes red if the
        // menu stops opening from the block (card moved away or
        // right-click routing broken). The branch line's own narrow menu
        // stays pinned by
        // `section_row_right_click_opens_project_member_targets_from_checkout_key`.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("linked");
        ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "grp".into(),
            label: "grp".into(),
            repo_root: "/repo".into(),
            checkout_path: "/repo/checkout".into(),
            is_linked_worktree: true,
        });
        app.state.workspaces = vec![ws];
        let branch_row = Rect::new(0, 5, 20, 1);
        let block = Rect::new(0, 6, 20, 2);
        app.state.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            ws_idx: 0,
            rect: block,
            indented: true,
        }];
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: branch_row,
            target: ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: "/repo/checkout".into(),
                collapse_key: "wsec:0".into(),
            },
        }];

        // Right-click on the branch line: NO workspace menu — the narrow
        // membership menu for the checkout dir opens instead.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Right),
                branch_row.x + 1,
                branch_row.y,
            ),
        );
        let branch_menu = app
            .state
            .context_menu
            .as_ref()
            .expect("right-click on the branch line still opens a menu");
        assert!(
            matches!(
                &branch_menu.kind,
                crate::app::state::ContextMenuKind::ProjectMemberTargets { .. }
            ),
            "the branch line's menu is the member-only one now: {:?}",
            branch_menu.kind
        );
        app.state.context_menu = None;
        app.state.mode = Mode::Terminal;

        // Right-click one row down, on the block: the FULL workspace menu.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Right),
                block.x + 1,
                block.y,
            ),
        );
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("right-click on the workspace block must open a context menu");
        assert!(
            matches!(
                &menu.kind,
                crate::app::state::ContextMenuKind::GitWorkspace { ws_idx: 0, .. }
            ),
            "expected the full workspace menu (GitWorkspace) from the block"
        );
        assert!(
            menu.items.iter().any(|item| item == "Rename"),
            "full workspace item missing: {:?}",
            menu.items
        );
        assert!(
            menu.items.iter().any(|item| item == "New project\u{2026}"),
            "project membership item missing: {:?}",
            menu.items
        );
    }

    #[test]
    fn project_view_section_row_and_pane_dots_row_menus_gain_section_controls_bora_79l_10() {
        // bora-79l.10 T6b: bead bora-79l.7 (F5) was supposed to land the
        // section-control items on BOTH the branch header row (SectionRow
        // -> ProjectMemberTargets) and the workspace's own block
        // (PaneDotsRow -> GitWorkspace) and did not. Red on either row if
        // the splice regresses, and red if a single item either row
        // already offered (membership "Remove", the workspace's "Rename")
        // drops out — proving nothing was traded away.
        use crate::config::IsolatedDirs;
        use crate::persist::projects::{self, Member, Project, WorktreesScope};

        let _isolated = IsolatedDirs::new("section-menu-controls-both-rows");
        let checkout = "/repo/checkout";
        projects::update_projects_file::<String>(move |file| {
            file.projects.insert(
                "alpha".to_string(),
                Project {
                    name: None,
                    channel: None,
                    members: vec![Member {
                        dir: checkout.to_string(),
                        worktrees: WorktreesScope::All,
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
        .expect("seed alpha project owning the checkout");

        let mut app = app_for_mouse_test();
        app.state.projects = projects::ProjectsStore::load();
        app.state.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("linked");
        ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "grp".into(),
            label: "grp".into(),
            repo_root: "/repo".into(),
            checkout_path: checkout.into(),
            is_linked_worktree: true,
        });
        app.state.workspaces = vec![ws];

        let branch_row = Rect::new(0, 5, 20, 1);
        let block = Rect::new(0, 6, 20, 2);
        app.state.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            ws_idx: 0,
            rect: block,
            indented: true,
        }];
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: branch_row,
            target: ProjectRowTarget::Section {
                ws_idx: 0,
                checkout_key: checkout.to_string(),
                collapse_key: "wsec:0".into(),
            },
        }];

        // The branch line: ProjectMemberTargets, keeps "Remove" AND gains
        // the section controls.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Right),
                branch_row.x + 1,
                branch_row.y,
            ),
        );
        let branch_menu = app
            .state
            .context_menu
            .as_ref()
            .expect("right-click on the branch line opens a menu");
        assert!(
            matches!(
                &branch_menu.kind,
                ContextMenuKind::ProjectMemberTargets { member_dir } if member_dir == checkout
            ),
            "kind: {:?}",
            branch_menu.kind
        );
        assert!(
            branch_menu.items.iter().any(|item| item == "Remove"),
            "the pre-existing membership item must survive: {:?}",
            branch_menu.items
        );
        for expected in [
            "Header: ON",
            "PART bolinhas: ON",
            "PART diff: ON",
            "Nova section: BRANCH",
            "Nova section: COMANDO",
            "Nova section: CHECKS",
            "Nova section: LIVRE",
        ] {
            assert!(
                branch_menu.items.iter().any(|item| item == expected),
                "branch line menu missing {expected:?}: {:?}",
                branch_menu.items
            );
        }
        app.state.context_menu = None;
        app.state.mode = Mode::Terminal;

        // The block: GitWorkspace, keeps "Rename" AND gains the same
        // section controls.
        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Down(MouseButton::Right),
                block.x + 1,
                block.y,
            ),
        );
        let block_menu = app
            .state
            .context_menu
            .as_ref()
            .expect("right-click on the block opens a menu");
        assert!(
            matches!(
                &block_menu.kind,
                ContextMenuKind::GitWorkspace { ws_idx: 0, .. }
            ),
            "kind: {:?}",
            block_menu.kind
        );
        assert!(
            block_menu.items.iter().any(|item| item == "Rename"),
            "the pre-existing workspace item must survive: {:?}",
            block_menu.items
        );
        for expected in [
            "Header: ON",
            "PART bolinhas: ON",
            "PART diff: ON",
            "Nova section: BRANCH",
            "Nova section: COMANDO",
            "Nova section: CHECKS",
            "Nova section: LIVRE",
        ] {
            assert!(
                block_menu.items.iter().any(|item| item == expected),
                "block menu missing {expected:?}: {:?}",
                block_menu.items
            );
        }
    }

    #[test]
    fn workspace_drag_dropped_on_declared_project_header_reparents_it() {
        // bora regression fix, item 5 ("navigate between different
        // groups"): dropping a dragged workspace onto a declared
        // project's header row moves it into that project via the
        // already-existing `Workspace::set_project`/`project()` binding —
        // no schema change, no `projects.yml` write from here.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        app.state.workspaces = vec![Workspace::test_new("a")];
        let project_header = Rect::new(0, 3, 20, 1);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: project_header,
            target: ProjectRowTarget::Project {
                collapse_key: "proj:alpha".into(),
            },
        }];
        app.state.drag = Some(DragState {
            target: DragTarget::WorkspaceReorder {
                source_id: crate::app::LOCAL_INPUT_SOURCE,
                source_ws_idx: 0,
                insert_idx: Some(0),
            },
        });
        app.state.force_full_repaint = false;

        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                project_header.x + 1,
                project_header.y,
            ),
        );

        assert_eq!(app.state.workspaces[0].project(), Some("alpha"));
        assert!(app.state.drag.is_none());
        assert!(
            app.state.force_full_repaint,
            "re-parenting reflows the list and must force a full repaint (AGENTS.md)"
        );
    }

    #[test]
    fn workspace_drag_dropped_on_orphan_project_header_clears_project() {
        // The inverse of the above: dropping onto the implicit
        // `declared: false` orphan bucket (no slug) clears the binding,
        // exactly what `set_project(None)` means.
        let mut app = app_for_mouse_test();
        app.state.view_mode = crate::config::ViewMode::Project;
        let mut ws = Workspace::test_new("a");
        ws.set_project(Some("alpha".to_string()));
        app.state.workspaces = vec![ws];
        let orphan_header = Rect::new(0, 3, 20, 1);
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: orphan_header,
            target: ProjectRowTarget::Project {
                collapse_key: crate::ui::ORPHANS_COLLAPSE_KEY.to_string(),
            },
        }];
        app.state.drag = Some(DragState {
            target: DragTarget::WorkspaceReorder {
                source_id: crate::app::LOCAL_INPUT_SOURCE,
                source_ws_idx: 0,
                insert_idx: Some(0),
            },
        });

        app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                orphan_header.x + 1,
                orphan_header.y,
            ),
        );

        assert_eq!(app.state.workspaces[0].project(), None);
    }

    #[test]
    fn workspace_drag_dropped_on_a_workspace_row_still_reorders_and_leaves_project_untouched() {
        // Distinguishes the two drops cleanly: a `Section` (workspace)
        // drop target must still reorder, and must never touch the
        // source's `project()` binding — only a `Project` (header) drop
        // target re-parents.
        let mut app = app_for_mouse_test();
        let mut ws_a = Workspace::test_new("a");
        ws_a.set_project(Some("alpha".to_string()));
        app.state.workspaces = vec![ws_a, Workspace::test_new("b")];
        app.state.active = Some(0);
        app.state.selected = 0;
        let area = Rect::new(0, 0, 106, 20);
        crate::ui::compute_view(&mut app.state, area);

        let second = app.state.view.workspace_card_areas[1].rect;
        // A stray Project-view `Section` hit area happens to sit over the
        // second card too — proves the drop distinguishes by TARGET KIND,
        // not merely by "some project_row_areas entry exists here".
        app.state.view.project_row_areas = vec![ProjectRowHitArea {
            rect: second,
            target: ProjectRowTarget::Section {
                ws_idx: 1,
                checkout_key: "/repo/checkout-b".into(),
                collapse_key: "wsec:1".into(),
            },
        }];
        app.state.drag = Some(DragState {
            target: DragTarget::WorkspaceReorder {
                source_id: crate::app::LOCAL_INPUT_SOURCE,
                source_ws_idx: 0,
                insert_idx: Some(2),
            },
        });

        let action = app.state.handle_mouse(
            &mut app.terminal_runtimes,
            crate::app::LOCAL_INPUT_SOURCE,
            mouse(
                MouseEventKind::Up(MouseButton::Left),
                second.x + 1,
                second.y,
            ),
        );

        assert_eq!(
            app.state.workspaces[0].project(),
            Some("alpha"),
            "dropping on a workspace row must not touch the source's project binding"
        );
        assert!(
            matches!(
                action,
                Some(MouseAction::MoveWorkspace {
                    source_ws_idx: 0,
                    insert_idx: 2,
                })
            ),
            "dropping on a workspace row must still reorder"
        );
    }

    fn dagr_test_plugin(enabled: bool) -> crate::api::schema::InstalledPluginInfo {
        crate::api::schema::InstalledPluginInfo {
            plugin_id: "dev.dagr".into(),
            name: "herdr-dagr".into(),
            version: "0.1.0".into(),
            min_herdr_version: String::new(),
            description: None,
            manifest_path: "/nonexistent".into(),
            plugin_root: "/nonexistent".into(),
            enabled,
            platforms: None,
            build: vec![],
            startup: vec![],
            actions: vec![crate::api::schema::PluginManifestAction {
                id: "open-dagr".into(),
                title: "Open dagr".into(),
                description: None,
                contexts: vec![crate::api::schema::PluginActionContext::Global],
                platforms: None,
                command: vec!["true".into()],
            }],
            events: vec![],
            panes: vec![],
            link_handlers: vec![],
            source: crate::api::schema::PluginSourceInfo::default(),
            warnings: vec![],
        }
    }

    #[test]
    fn plugin_action_context_dagr_via_general_mechanism_still_offers_entry() {
        // bora-1e9: dagr's hardcoded special case (a dedicated action-id
        // constant, an availability flag on the menu kind, the
        // channels-only gate) is gone. Its manifest
        // declaring `contexts = ["global"]` is now the ONLY thing that puts
        // "Open dagr" on a group-header menu — proved through the real
        // mouse-click handler, not build_context_menu_items directly, so
        // this exercises the actual construction site in this file.
        let mut app = app_for_mouse_test();
        app.state.view.workspace_group_header_areas = vec![
            crate::app::state::GroupHeaderCardArea {
                name: "channels".into(),
                collapse_key: "vg:channels".into(),
                rect: Rect::new(0, 2, 20, 1),
            },
            crate::app::state::GroupHeaderCardArea {
                name: "side-quest".into(),
                collapse_key: "vg:side-quest".into(),
                rect: Rect::new(0, 3, 20, 1),
            },
        ];

        let reset = |app: &mut crate::app::App| {
            app.state.context_menu = None;
            app.state.mode = Mode::Terminal;
        };

        // Registered and enabled: Global means every group header offers
        // it, not just the channels one (that restriction was the special
        // case being deleted).
        app.state
            .installed_plugins
            .insert("dev.dagr".into(), dagr_test_plugin(true));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 2, 2));
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("channels group menu");
        assert!(
            menu.items.iter().any(|item| item == "Open dagr"),
            "entry must appear when the dagr plugin is registered: {:?}",
            menu.items
        );

        reset(&mut app);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 2, 3));
        let menu = app.state.context_menu.as_ref().expect("other group menu");
        assert!(
            menu.items.iter().any(|item| item == "Open dagr"),
            "Global means every group header, not just channels: {:?}",
            menu.items
        );

        // Plugin disabled: registry present but not enabled reads as absent.
        reset(&mut app);
        app.state
            .installed_plugins
            .insert("dev.dagr".into(), dagr_test_plugin(false));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 2, 2));
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("channels group menu");
        assert!(
            !menu.items.iter().any(|item| item == "Open dagr"),
            "a disabled install is not an available one"
        );

        // No plugin at all: same silent skip, menu still opens normally.
        reset(&mut app);
        app.state.installed_plugins.clear();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 2, 2));
        let menu = app
            .state
            .context_menu
            .as_ref()
            .expect("channels group menu");
        assert!(
            !menu.items.iter().any(|item| item == "Open dagr"),
            "absent plugin means no entry — and no error, the menu still opens"
        );
    }
}
