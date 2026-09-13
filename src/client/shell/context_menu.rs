use super::*;

impl ClientContextMenuOverlay {
    pub(super) fn items(&self) -> Vec<ClientContextMenuItem> {
        use ClientContextMenuAction as Action;

        let item = |label: &'static str, action| ClientContextMenuItem {
            label: std::borrow::Cow::Borrowed(label),
            action,
        };
        match &self.target {
            ClientContextMenuTarget::Workspace {
                is_git,
                is_linked_worktree,
                has_worktree_children,
                collapsed,
                visual_group,
                group_paths,
                ..
            } => {
                let mut items = vec![item("Rename", Action::Rename)];
                if !*is_git {
                    items.push(item("Close", Action::Close));
                } else if *is_linked_worktree {
                    items.extend([
                        item("Close", Action::Close),
                        item("Delete worktree checkout...", Action::RemoveWorktree),
                    ]);
                } else if *has_worktree_children {
                    items.extend([
                        item("Close group", Action::Close),
                        item("New worktree", Action::NewWorktree),
                        item("Open worktree...", Action::OpenWorktree),
                        item(
                            if *collapsed { "Expand" } else { "Collapse" },
                            Action::ToggleGroup,
                        ),
                    ]);
                } else {
                    items.extend([
                        item("Close", Action::Close),
                        item("New worktree", Action::NewWorktree),
                        item("Open worktree...", Action::OpenWorktree),
                    ]);
                }
                // ceo-bora#303: this shell has no nested popup, so
                // "move to group" is a flat run of one item per known
                // group path rather than a submenu.
                items.push(item("New group…", Action::NewGroup));
                items.extend(group_paths.iter().map(|path| ClientContextMenuItem {
                    label: std::borrow::Cow::Owned(format!("→ {path}")),
                    action: Action::MoveToGroup(path.clone()),
                }));
                if visual_group.is_some() {
                    items.extend([
                        item("Rename group…", Action::RenameGroup),
                        item("Remove from group", Action::RemoveFromGroup),
                    ]);
                }
                items
            }
            ClientContextMenuTarget::GroupHeader { collapsed, .. } => vec![
                item("Rename group…", Action::RenameGroup),
                item(
                    if *collapsed { "Expand" } else { "Collapse" },
                    Action::ToggleGroup,
                ),
                item("Ungroup all", Action::UngroupAll),
                item("New workspace in group", Action::NewWorkspaceInGroup),
            ],
            ClientContextMenuTarget::Tab { .. } => vec![
                item("New tab", Action::NewTab),
                item("Rename", Action::Rename),
                item("Close", Action::Close),
            ],
            ClientContextMenuTarget::Pane {
                source_pane_id,
                has_manual_label,
                right_click_passthrough,
                ..
            } => {
                let mut items = vec![item("Rename pane", Action::RenamePane)];
                if *has_manual_label {
                    items.push(item("Clear pane name", Action::ClearPaneName));
                }
                items.push(item("Copy reference", Action::CopyPaneReference));
                if source_pane_id.is_some() {
                    items.push(item("Swap with focused pane", Action::SwapWithFocusedPane));
                }
                items.extend([
                    item("Split right", Action::SplitRight),
                    item("Split down", Action::SplitDown),
                    item("Zoom", Action::Zoom),
                    item(
                        if *right_click_passthrough {
                            "Use Herdr right-click menu"
                        } else {
                            "Send right-clicks to pane"
                        },
                        Action::ToggleRightClickPassthrough,
                    ),
                    item("Close pane", Action::ClosePane),
                ]);
                items
            }
        }
    }
}

impl ClientShellState {
    pub(super) fn open_workspace_context_menu(&mut self, workspace_id: String, x: u16, y: u16) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(workspace) = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)
        else {
            return;
        };
        let worktree = workspace.worktree.as_ref();
        // "Close group"/"Collapse" only make sense where the group is
        // actually rendered nested -- `ViewMode::Repo`. In Folders/Flat
        // the parent is a plain row (and Folders collapse keys are
        // `vg:`-prefixed), so it gets the plain workspace menu.
        let has_worktree_children = self.close_drags_worktree_group()
            && worktree.is_some_and(|worktree| {
                !worktree.is_linked_worktree
                    && snapshot
                        .workspaces
                        .iter()
                        .filter(|candidate| {
                            candidate
                                .worktree
                                .as_ref()
                                .is_some_and(|candidate| candidate.key == worktree.key)
                        })
                        .count()
                        >= 2
            });
        let collapsed = worktree.is_some_and(|worktree| {
            self.group_is_collapsed(&self.active_endpoint_id, &worktree.key)
        });
        let visual_group = workspace
            .visual_group
            .as_deref()
            .and_then(super::sidebar::normalize_group_path);
        // Every VISIBLE folder, including a parent that exists only as a
        // synthesized ancestor of a nested path -- not just paths with a
        // direct exact member (cubic review, ceo-bora#303 PR #32). Sorted
        // + deduped so the flattened "move to group" run is stable across
        // frames and across right-clicks.
        let (_, mut group_paths) = super::sidebar::folders_group_tree(snapshot);
        group_paths.sort();
        group_paths.dedup();
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Workspace {
                workspace_id,
                is_git: worktree.is_some() || workspace.branch.is_some(),
                is_linked_worktree: worktree.is_some_and(|worktree| worktree.is_linked_worktree),
                has_worktree_children,
                collapsed,
                visual_group,
                group_paths,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    /// ceo-bora#303: right-click on a Folders group header. `path` is
    /// the plain group path; the collapse key adds the `vg:` prefix.
    pub(super) fn open_group_context_menu(&mut self, path: String, x: u16, y: u16) {
        let collapsed = self.group_is_collapsed(&self.active_endpoint_id, &format!("vg:{path}"));
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::GroupHeader { path, collapsed },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn open_tab_context_menu(&mut self, tab_id: String, x: u16, y: u16) {
        let Some(tab) = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id))
        else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Tab {
                tab_id,
                workspace_id: tab.workspace_id.clone(),
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn open_pane_context_menu(&mut self, pane_id: String, x: u16, y: u16) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == pane_id) else {
            return;
        };
        let source_pane_id = snapshot
            .focused_pane_id
            .clone()
            .filter(|focused| focused != &pane_id);
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Pane {
                pane_id,
                workspace_id: pane.workspace_id.clone(),
                source_pane_id,
                has_manual_label: pane.label.is_some(),
                right_click_passthrough: pane.right_click_passthrough,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn move_context_menu_selection(&mut self, delta: isize) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        let item_count = menu.items().len();
        if item_count == 0 {
            return;
        }
        menu.highlighted = (menu.highlighted as isize + delta)
            .clamp(0, item_count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn activate_context_menu_item(
        &mut self,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.take() else {
            return;
        };
        let Some(action) = menu.items().get(index).map(|item| item.action.clone()) else {
            outcome.repaint = true;
            return;
        };
        match menu.target {
            ClientContextMenuTarget::Workspace { workspace_id, .. } => {
                self.activate_workspace_context_action(workspace_id, action, outcome)
            }
            ClientContextMenuTarget::GroupHeader { path, .. } => {
                self.activate_group_header_context_action(path, action, outcome)
            }
            ClientContextMenuTarget::Tab {
                tab_id,
                workspace_id,
            } => self.activate_tab_context_action(tab_id, workspace_id, action, outcome),
            ClientContextMenuTarget::Pane {
                pane_id,
                workspace_id,
                source_pane_id,
                right_click_passthrough,
                ..
            } => self.activate_pane_context_action(
                pane_id,
                workspace_id,
                source_pane_id,
                right_click_passthrough,
                action,
                outcome,
            ),
        }
        outcome.repaint = true;
    }

    fn activate_workspace_context_action(
        &mut self,
        workspace_id: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::input::KeybindAction;

        match action {
            ClientContextMenuAction::Rename => {
                let label = self
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| {
                        snapshot
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.workspace_id == workspace_id)
                    })
                    .map(|workspace| workspace.label.clone());
                if let Some(label) = label {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename workspace",
                        input: label,
                        replace_on_type: false,
                        target: ClientRenameTarget::Workspace { workspace_id },
                    }));
                }
            }
            ClientContextMenuAction::Close => {
                if self.config.confirm_close {
                    self.open_confirm_close_overlay(workspace_id);
                } else {
                    self.push_endpoint_method(
                        crate::api::schema::Method::WorkspaceClose(
                            crate::api::schema::WorkspaceCloseParams {
                                workspace_id,
                                close_group: self.close_drags_worktree_group(),
                            },
                        ),
                        outcome,
                    );
                }
            }
            ClientContextMenuAction::NewWorktree => {
                self.begin_worktree_action_for(KeybindAction::NewWorktree, workspace_id, outcome)
            }
            ClientContextMenuAction::OpenWorktree => {
                self.begin_worktree_action_for(KeybindAction::OpenWorktree, workspace_id, outcome)
            }
            ClientContextMenuAction::RemoveWorktree => {
                self.begin_worktree_action_for(KeybindAction::RemoveWorktree, workspace_id, outcome)
            }
            ClientContextMenuAction::ToggleGroup => {
                let key = self.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.workspace_id == workspace_id)
                        .and_then(|workspace| workspace.worktree.as_ref())
                        .map(|worktree| worktree.key.clone())
                });
                if let Some(key) = key {
                    let endpoint_id = self.active_endpoint_id.clone();
                    self.toggle_collapsed_group(&endpoint_id, key);
                    self.persist_chrome_preferences(outcome);
                }
            }
            ClientContextMenuAction::NewGroup => {
                let parent_path = self.workspace_visual_group(&workspace_id);
                // Seeded with the workspace's own path plus a
                // separator, so confirming after typing a leaf nests
                // the new group under the current one (ceo-bora#303).
                let input = parent_path
                    .as_deref()
                    .map(|parent| format!("{parent}/"))
                    .unwrap_or_default();
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "new group",
                    input,
                    replace_on_type: false,
                    target: ClientRenameTarget::NewGroup {
                        workspace_id,
                        parent_path,
                    },
                }));
            }
            ClientContextMenuAction::MoveToGroup(path) => self.push_endpoint_method(
                crate::api::schema::Method::WorkspaceSetGroup(
                    crate::api::schema::WorkspaceSetGroupParams {
                        workspace_id,
                        group: Some(path),
                    },
                ),
                outcome,
            ),
            ClientContextMenuAction::RenameGroup => {
                if let Some(old_path) = self.workspace_visual_group(&workspace_id) {
                    self.open_group_rename_overlay(old_path);
                }
            }
            ClientContextMenuAction::RemoveFromGroup => self.push_endpoint_method(
                crate::api::schema::Method::WorkspaceSetGroup(
                    crate::api::schema::WorkspaceSetGroupParams {
                        workspace_id,
                        group: None,
                    },
                ),
                outcome,
            ),
            _ => {}
        }
    }

    fn activate_group_header_context_action(
        &mut self,
        path: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        match action {
            ClientContextMenuAction::RenameGroup => self.open_group_rename_overlay(path),
            ClientContextMenuAction::ToggleGroup => {
                let endpoint_id = self.active_endpoint_id.clone();
                self.toggle_collapsed_group(&endpoint_id, format!("vg:{path}"));
                self.persist_chrome_preferences(outcome);
            }
            ClientContextMenuAction::UngroupAll => {
                for method in self.group_repath_methods(&path, None) {
                    self.push_endpoint_method(method, outcome);
                }
            }
            ClientContextMenuAction::NewWorkspaceInGroup => {
                if self.config.prompt_new_workspace_name {
                    self.open_new_workspace_overlay_with_group(Some(path));
                } else {
                    self.push_endpoint_method(
                        crate::api::schema::Method::WorkspaceCreate(
                            crate::api::schema::WorkspaceCreateParams {
                                group: Some(path),
                                source_workspace_id: None,
                                cwd: None,
                                focus: true,
                                label: None,
                                env: Default::default(),
                            },
                        ),
                        outcome,
                    );
                }
            }
            _ => {}
        }
    }

    fn workspace_visual_group(&self, workspace_id: &str) -> Option<String> {
        self.snapshot
            .as_deref()?
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)?
            .visual_group
            .as_deref()
            .and_then(super::sidebar::normalize_group_path)
    }

    /// Prompt for the last segment of `old_path`; the parent prefix is
    /// kept as-is and re-applied on save (ceo-bora#303).
    fn open_group_rename_overlay(&mut self, old_path: String) {
        let leaf = old_path.rsplit('/').next().unwrap_or_default().to_owned();
        self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            title: "rename group",
            input: leaf,
            replace_on_type: false,
            target: ClientRenameTarget::Group { old_path },
        }));
    }

    /// One `WorkspaceSetGroup` per workspace sitting at `path` or nested
    /// under it. `replacement` swaps that prefix (keeping each workspace's
    /// own relative suffix); `None` ungroups them all. Matches against each
    /// workspace's NORMALIZED `visual_group` (`sidebar::normalize_group_path`),
    /// not the raw wire string -- `path` itself, coming from a rendered
    /// header or `workspace_visual_group`, is already normalized, and a raw
    /// value like `"foo//bar"` must still match the canonical `"foo/bar"`
    /// header it renders under, or renaming/ungrouping from that header
    /// silently leaves it untouched (cubic review, ceo-bora#303 PR #32).
    pub(super) fn group_repath_methods(
        &self,
        path: &str,
        replacement: Option<&str>,
    ) -> Vec<crate::api::schema::Method> {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return Vec::new();
        };
        let prefix = format!("{path}/");
        snapshot
            .workspaces
            .iter()
            .filter_map(|workspace| {
                let group = workspace
                    .visual_group
                    .as_deref()
                    .and_then(super::sidebar::normalize_group_path)?;
                let suffix = if group == path {
                    None
                } else {
                    Some(group.strip_prefix(prefix.as_str())?.to_owned())
                };
                let group = replacement.map(|replacement| match &suffix {
                    Some(suffix) => format!("{replacement}/{suffix}"),
                    None => replacement.to_owned(),
                });
                Some(crate::api::schema::Method::WorkspaceSetGroup(
                    crate::api::schema::WorkspaceSetGroupParams {
                        workspace_id: workspace.workspace_id.clone(),
                        group,
                    },
                ))
            })
            .collect()
    }

    fn activate_tab_context_action(
        &mut self,
        tab_id: String,
        workspace_id: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{Method, TabTarget};

        self.push_endpoint_method(
            Method::TabFocus(TabTarget {
                tab_id: tab_id.clone(),
            }),
            outcome,
        );
        match action {
            ClientContextMenuAction::NewTab => {
                if self.config.prompt_new_tab_name {
                    let default_name = (self
                        .snapshot
                        .as_deref()
                        .map(|snapshot| {
                            snapshot
                                .tabs
                                .iter()
                                .filter(|tab| tab.workspace_id == workspace_id)
                                .count()
                        })
                        .unwrap_or(0)
                        + 1)
                    .to_string();
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "new tab",
                        input: default_name.clone(),
                        replace_on_type: true,
                        target: ClientRenameTarget::NewTab {
                            workspace_id,
                            default_name,
                        },
                    }));
                } else {
                    self.push_endpoint_method(
                        Method::TabCreate(crate::api::schema::TabCreateParams {
                            workspace_id: Some(workspace_id),
                            cwd: None,
                            focus: true,
                            label: None,
                            env: Default::default(),
                        }),
                        outcome,
                    );
                }
            }
            ClientContextMenuAction::Rename => {
                let tab = self
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id));
                if let Some(tab) = tab {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename tab",
                        input: tab.label.clone(),
                        replace_on_type: false,
                        target: ClientRenameTarget::Tab {
                            tab_id,
                            auto_name: !tab.custom_label,
                            original_name: tab.label.clone(),
                        },
                    }));
                }
            }
            ClientContextMenuAction::Close => {
                self.push_endpoint_method(Method::TabClose(TabTarget { tab_id }), outcome);
            }
            _ => {}
        }
    }

    fn activate_pane_context_action(
        &mut self,
        pane_id: String,
        workspace_id: String,
        source_pane_id: Option<String>,
        right_click_passthrough: bool,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{
            Method, PaneInputSetParams, PaneRenameParams, PaneRightClickTarget, PaneSplitParams,
            PaneSwapParams, PaneTarget, PaneZoomMode, PaneZoomParams, SplitDirection,
        };

        match action {
            ClientContextMenuAction::RenamePane => {
                let label = self.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .panes
                        .iter()
                        .find(|pane| pane.pane_id == pane_id)
                        .and_then(|pane| pane.label.clone())
                });
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename pane",
                    input: label.clone().unwrap_or_default(),
                    replace_on_type: label.is_none(),
                    target: ClientRenameTarget::Pane { pane_id },
                }));
            }
            ClientContextMenuAction::ClearPaneName => self.push_endpoint_method(
                Method::PaneRename(PaneRenameParams {
                    pane_id,
                    label: None,
                }),
                outcome,
            ),
            // ceo-bora#315: `<workspace label> <pane_id>` is the form the
            // `bora agent prompt <pane>` / channel workflows paste, so the
            // label is resolved from the live snapshot at activation and the
            // bytes go out through the same ClipboardWrite path as
            // selection copy (`PendingEndpointKind::SelectionCopy`).
            ClientContextMenuAction::CopyPaneReference => {
                let label = self.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.workspace_id == workspace_id)
                        .map(|workspace| workspace.label.as_str())
                });
                let reference = match label {
                    Some(label) => format!("{label} {pane_id}"),
                    None => pane_id,
                };
                self.show_copy_feedback(std::time::Instant::now());
                outcome
                    .actions
                    .push(ClientShellAction::ClipboardWrite(reference.into_bytes()));
            }
            ClientContextMenuAction::SwapWithFocusedPane => {
                if let Some(source_pane_id) = source_pane_id {
                    self.push_endpoint_method(
                        Method::PaneSwap(PaneSwapParams {
                            pane_id: None,
                            direction: None,
                            source_pane_id: Some(source_pane_id.clone()),
                            target_pane_id: Some(pane_id),
                        }),
                        outcome,
                    );
                    self.push_endpoint_method(
                        Method::PaneFocus(PaneTarget {
                            pane_id: source_pane_id,
                        }),
                        outcome,
                    );
                }
            }
            ClientContextMenuAction::SplitRight | ClientContextMenuAction::SplitDown => {
                self.push_endpoint_method(
                    Method::PaneSplit(PaneSplitParams {
                        workspace_id: Some(workspace_id),
                        target_pane_id: Some(pane_id),
                        direction: if action == ClientContextMenuAction::SplitRight {
                            SplitDirection::Right
                        } else {
                            SplitDirection::Down
                        },
                        ratio: None,
                        cwd: None,
                        focus: true,
                        right_click: Default::default(),
                        env: Default::default(),
                    }),
                    outcome,
                );
            }
            ClientContextMenuAction::Zoom => self.push_endpoint_method(
                Method::PaneZoom(PaneZoomParams {
                    pane_id: Some(pane_id),
                    mode: PaneZoomMode::Toggle,
                }),
                outcome,
            ),
            ClientContextMenuAction::ToggleRightClickPassthrough => self.push_endpoint_method(
                Method::PaneInputSet(PaneInputSetParams {
                    pane_id,
                    right_click: if right_click_passthrough {
                        PaneRightClickTarget::Herdr
                    } else {
                        PaneRightClickTarget::Pane
                    },
                }),
                outcome,
            ),
            ClientContextMenuAction::ClosePane => {
                self.push_endpoint_method(Method::PaneClose(PaneTarget { pane_id }), outcome)
            }
            _ => {}
        }
    }
}
