use super::*;

#[test]
fn tab_overflow_controls_scroll_the_client_owned_tab_bar() {
    let mut snapshot = snapshot();
    snapshot.tabs.extend((2..=8).map(|number| ClientShellTab {
        tab_id: format!("tab_{number}"),
        workspace_id: "ws_1".into(),
        number,
        label: number.to_string(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    }));
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    state.compose(80, 20).expect("overflow tab bar");

    assert!(state.hits.tab_scroll_right.width > 0);
    let scroll_right = state.hits.tab_scroll_right;
    let outcome =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: scroll_right.x + 1,
            row: scroll_right.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(outcome.repaint);
    assert_eq!(state.tab_scroll, 1);

    let mut update = state.snapshot.as_deref().expect("snapshot").clone();
    update.focused_tab_id = Some("tab_8".into());
    for tab in &mut update.tabs {
        tab.focused = tab.tab_id == "tab_8";
    }
    state.set_snapshot(Box::new(update));
    state.compose(80, 20).expect("focused overflow tab");
    assert!(state.hits.tabs.iter().any(|(_, tab_id)| tab_id == "tab_8"));

    state.compose(300, 20).expect("tabs without overflow");
    assert_eq!(state.tab_scroll, 0);
    assert_eq!(state.hits.tabs.len(), 8);
    state.compose(80, 20).expect("focused tab after narrowing");
    assert!(state.hits.tabs.iter().any(|(_, tab_id)| tab_id == "tab_8"));
}

#[test]
fn focused_workspace_change_reveals_new_workspace_in_full_sidebar() {
    let mut initial = snapshot();
    let template = initial.workspaces[0].clone();
    initial.workspaces = (1..=12)
        .map(|number| ClientShellWorkspace {
            workspace_id: format!("ws_{number}"),
            number,
            label: format!("space-{number}"),
            branch: None,
            focused: number == 1,
            ..template.clone()
        })
        .collect();

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(initial));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("full sidebar");
    assert!(state.hits.workspace_max_scroll > 0);
    assert!(state
        .hits
        .workspaces
        .iter()
        .all(|hit| hit.workspace_id != "ws_12"));

    let mut update = state.snapshot.as_deref().expect("snapshot").clone();
    update.revision = 2;
    update.focused_workspace_id = Some("ws_12".into());
    for workspace in &mut update.workspaces {
        workspace.focused = workspace.workspace_id == "ws_12";
    }
    let mut updated_surface = surface();
    updated_surface.projection_revision = 2;
    state.set_snapshot(Box::new(update));
    state.set_pane_surface(updated_surface);
    state.compose(106, 2).expect("zero-height workspace body");
    assert!(state.reveal_focused_workspace);
    state.compose(106, 20).expect("updated full sidebar");

    assert!(state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == "ws_12"));
}

#[test]
fn client_owned_sidebar_dividers_resize_live() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("expanded sidebar");
    let workspace_body = state.hits.workspace_body;
    let needless_scroll =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: workspace_body.x,
            row: workspace_body.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert_eq!(state.hits.workspace_max_scroll, 0);
    assert_eq!(state.workspace_scroll, 0);
    assert!(!needless_scroll.repaint);
    let width_divider = state.hits.sidebar_divider;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: width_divider.x,
        row: width_divider.y + 2,
        modifiers: KeyModifiers::empty(),
    })]);
    let resize =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 31,
            row: width_divider.y + 2,
            modifiers: KeyModifiers::empty(),
        })]);
    assert_eq!(state.sidebar_width, 32);
    assert!(state.sidebar_width_manual);
    assert!(resize.repaint);
    assert!(resize.resize);
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: 31,
        row: width_divider.y + 2,
        modifiers: KeyModifiers::empty(),
    })]);

    state.set_pane_surface(surface());
    state.compose(106, 30).expect("resized sidebar");
    let section_divider = state.hits.sidebar_section_divider;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: section_divider.x + 2,
        row: section_divider.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let split = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: section_divider.x + 2,
        row: 20,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.sidebar_section_split > 0.6);
    assert!(split.repaint);
    assert!(!split.resize);
}

#[test]
fn context_menus_capture_stable_targets_and_route_actions() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");

    let workspace = state.hits.workspaces[0].rect;
    let open_workspace_menu =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: workspace.x + 2,
            row: workspace.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(open_workspace_menu.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Workspace { ref workspace_id, .. },
            ..
        })) if workspace_id == "ws_1"
    ));
    let workspace_items = match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu.items(),
        _ => panic!("workspace context menu"),
    };
    assert!(workspace_items
        .iter()
        .any(|item| item.action == ClientContextMenuAction::NewWorktree));
    state.compose(106, 20).expect("workspace context menu");
    let rename = state.hits.context_menu_rows[0].0;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rename.x + 1,
        row: rename.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::Workspace { ref workspace_id },
            ..
        })) if workspace_id == "ws_1"
    ));

    state.overlay = None;
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].rect;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: pane.x + 1,
        row: pane.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(106, 20).expect("pane context menu");
    let split_index = match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu
            .items()
            .iter()
            .position(|item| item.action == ClientContextMenuAction::SplitRight)
            .expect("split right item"),
        _ => panic!("pane context menu"),
    };
    let split = state.hits.context_menu_rows[split_index].0;
    let outcome =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: split.x + 1,
            row: split.y,
            modifiers: KeyModifiers::empty(),
        })]);
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("pane split context action should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneSplit(params)
            if params.target_pane_id.as_deref() == Some("pane_1")
                && params.direction == crate::api::schema::SplitDirection::Right
    ));
}

#[test]
fn global_menu_opens_from_sidebar_and_routes_client_actions() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("shell frame");
    let launcher = state.hits.global_launcher;
    assert_ne!(launcher, Rect::default());

    let open = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: launcher.x,
        row: launcher.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(open.repaint);
    let menu = state.compose(106, 30).expect("global menu");
    let text = menu
        .cells
        .chunks(menu.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("settings"));
    assert!(text.contains("keybinds"));
    assert!(text.contains("reload config"));
    assert!(text.contains("detach"));

    let keybinds = state.hits.global_menu_rows[1].0;
    let help = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: keybinds.x,
        row: keybinds.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(help.actions.is_empty());
    assert!(matches!(state.overlay, Some(ClientShellOverlay::Help(_))));

    state.overlay = Some(ClientShellOverlay::GlobalMenu(ClientGlobalMenuOverlay {
        highlighted: 3,
    }));
    let detach = state.handle_input_bytes(b"\r");
    assert!(detach.detach);
    assert!(state.overlay.is_none());
}

#[test]
fn new_tab_overlay_owns_text_cursor_and_submits_public_api_request() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewTab),
        &mut open,
    );
    assert!(open.actions.is_empty());
    let frame = state.compose(106, 20).expect("new tab overlay");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("new tab"));
    assert!(text.contains("save"));
    let restored = frame.to_ratatui_buffer().expect("overlay frame");
    assert!(!restored
        .cell((26, 7))
        .expect("overlay title cell")
        .modifier
        .contains(Modifier::DIM));
    assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));

    assert!(state.handle_input_bytes(b"logs").actions.is_empty());
    let create = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &create.actions[..] else {
        panic!("new tab save should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::TabCreate(params)
            if params.workspace_id.as_deref() == Some("ws_1")
                && params.label.as_deref() == Some("logs")
    ));
    assert!(state.overlay.is_none());
}

#[test]
fn close_confirmation_error_becomes_client_owned_overlay_and_stable_group_close() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut close = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ClosePane),
        &mut close,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &close.actions[..] else {
        panic!("pane close should use endpoint API");
    };
    let request_id = request.id.clone();
    assert!(
        state
            .handle_endpoint_result(
                "boot-1",
                &request_id,
                Err(ClientShellEndpointError {
                    code: Some("confirmation_required".into()),
                    message: "confirmation required".into(),
                }),
            )
            .0
    );
    let frame = state.compose(106, 20).expect("confirmation overlay");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Close workspace?"));
    assert!(text.contains("1 pane"));

    let confirm = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &confirm.actions[..] else {
        panic!("confirmation should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceClose(params)
            if params.workspace_id == "ws_1" && params.close_group
    ));
}

/// ceo-bora#303 group context menus. Folders view, one top-level group
/// (`bora-sync`) with a nested subgroup (`bora-sync/docs`) and one
/// ungrouped workspace, so both the "has a group" and "has no group"
/// menus are reachable from the same snapshot.
fn grouped_folders_state(prompt_new_workspace_name: bool) -> ClientShellState {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    config.ui.prompt_new_workspace_name = prompt_new_workspace_name;

    let mut projected = snapshot();
    projected.workspaces[0].visual_group = Some("bora-sync".into());
    let mut nested = projected.workspaces[0].clone();
    nested.workspace_id = "ws_2".into();
    nested.number = 2;
    nested.label = "docs-space".into();
    nested.focused = false;
    nested.visual_group = Some("bora-sync/docs".into());
    let mut loose = projected.workspaces[0].clone();
    loose.workspace_id = "ws_3".into();
    loose.number = 3;
    loose.label = "loose-space".into();
    loose.focused = false;
    loose.visual_group = None;
    projected.workspaces.push(nested);
    projected.workspaces.push(loose);

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("folders layout");
    state
}

fn right_click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn menu_actions(state: &ClientShellState) -> Vec<ClientContextMenuAction> {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => {
            menu.items().into_iter().map(|item| item.action).collect()
        }
        _ => panic!("expected a context menu"),
    }
}

fn menu_labels(state: &ClientShellState) -> Vec<String> {
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu
            .items()
            .into_iter()
            .map(|item| item.label.into_owned())
            .collect(),
        _ => panic!("expected a context menu"),
    }
}

/// Click the menu row carrying `action`, exercising the same hit path a
/// user takes instead of calling the activation helper directly.
fn click_menu_item(
    state: &mut ClientShellState,
    action: &ClientContextMenuAction,
) -> ClientShellInput {
    let index = menu_actions(state)
        .iter()
        .position(|candidate| candidate == action)
        .unwrap_or_else(|| panic!("menu item {action:?} in {:?}", menu_labels(state)));
    state.compose(106, 30).expect("context menu frame");
    let row = state.hits.context_menu_rows[index].0;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: row.x + 1,
        row: row.y,
        modifiers: KeyModifiers::empty(),
    })])
}

fn workspace_rect(state: &ClientShellState, workspace_id: &str) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("workspace row")
        .rect
}

fn group_header_rect(state: &ClientShellState, collapse_key: &str) -> Rect {
    state
        .hits
        .folders_group_headers
        .iter()
        .find(|(_, key)| key.as_str() == collapse_key)
        .map(|(rect, _)| *rect)
        .expect("group header row")
}

fn set_group_methods(outcome: &ClientShellInput) -> Vec<(String, Option<String>)> {
    outcome
        .actions
        .iter()
        .map(|action| {
            let ClientShellAction::Endpoint { request, .. } = action else {
                panic!("group actions should use the endpoint API: {action:?}");
            };
            match &request.method {
                crate::api::schema::Method::WorkspaceSetGroup(params) => {
                    (params.workspace_id.clone(), params.group.clone())
                }
                other => panic!("expected WorkspaceSetGroup, got {other:?}"),
            }
        })
        .collect()
}

#[test]
fn workspace_context_menu_moves_an_ungrouped_workspace_into_an_existing_group() {
    let mut state = grouped_folders_state(false);
    let loose = workspace_rect(&state, "ws_3");
    right_click(&mut state, loose);

    let labels = menu_labels(&state);
    assert!(labels.contains(&"New group…".to_owned()), "{labels:?}");
    // Flattened "move to group" run: one row per distinct path, sorted.
    assert!(labels.contains(&"→ bora-sync".to_owned()), "{labels:?}");
    assert!(
        labels.contains(&"→ bora-sync/docs".to_owned()),
        "{labels:?}"
    );
    // An ungrouped workspace has no group to rename or leave.
    assert!(!labels.contains(&"Rename group…".to_owned()), "{labels:?}");
    assert!(
        !labels.contains(&"Remove from group".to_owned()),
        "{labels:?}"
    );

    let moved = click_menu_item(
        &mut state,
        &ClientContextMenuAction::MoveToGroup("bora-sync".into()),
    );
    assert_eq!(
        set_group_methods(&moved),
        vec![("ws_3".to_owned(), Some("bora-sync".to_owned()))]
    );
}

#[test]
fn workspace_context_menu_new_group_nests_under_the_workspace_own_group() {
    let mut state = grouped_folders_state(false);
    let grouped = workspace_rect(&state, "ws_1");
    right_click(&mut state, grouped);
    click_menu_item(&mut state, &ClientContextMenuAction::NewGroup);

    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_ref() else {
        panic!("new group prompt");
    };
    // Seeded with the workspace's own path as an editable prefix.
    assert_eq!(rename.input, "bora-sync/");
    assert!(matches!(
        &rename.target,
        ClientRenameTarget::NewGroup {
            workspace_id,
            parent_path: Some(parent),
        } if workspace_id == "ws_1" && parent == "bora-sync"
    ));

    let created = state.handle_input_bytes(b"docs\r");
    assert_eq!(
        set_group_methods(&created),
        vec![("ws_1".to_owned(), Some("bora-sync/docs".to_owned()))]
    );

    // Confirming the bare prefix must not create a group whose last
    // segment is empty, nor re-set the group the workspace already has.
    right_click(&mut state, grouped);
    click_menu_item(&mut state, &ClientContextMenuAction::NewGroup);
    let unchanged = state.handle_input_bytes(b"\r");
    assert!(unchanged.actions.is_empty(), "{:?}", unchanged.actions);
}

#[test]
fn workspace_context_menu_new_group_from_an_ungrouped_workspace_starts_empty() {
    let mut state = grouped_folders_state(false);
    let loose = workspace_rect(&state, "ws_3");
    right_click(&mut state, loose);
    click_menu_item(&mut state, &ClientContextMenuAction::NewGroup);

    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_ref() else {
        panic!("new group prompt");
    };
    assert!(rename.input.is_empty());
    assert!(matches!(
        &rename.target,
        ClientRenameTarget::NewGroup {
            parent_path: None,
            ..
        }
    ));

    let created = state.handle_input_bytes(b"alpha\r");
    assert_eq!(
        set_group_methods(&created),
        vec![("ws_3".to_owned(), Some("alpha".to_owned()))]
    );
}

#[test]
fn workspace_context_menu_remove_from_group_clears_only_that_workspace() {
    let mut state = grouped_folders_state(false);
    let grouped = workspace_rect(&state, "ws_1");
    right_click(&mut state, grouped);

    let outcome = click_menu_item(&mut state, &ClientContextMenuAction::RemoveFromGroup);
    assert_eq!(set_group_methods(&outcome), vec![("ws_1".to_owned(), None)]);
}

#[test]
fn workspace_context_menu_rename_group_repaths_members_and_nested_subgroups() {
    let mut state = grouped_folders_state(false);
    let grouped = workspace_rect(&state, "ws_1");
    right_click(&mut state, grouped);
    click_menu_item(&mut state, &ClientContextMenuAction::RenameGroup);

    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_ref() else {
        panic!("group rename prompt");
    };
    assert_eq!(rename.input, "bora-sync");
    assert!(matches!(
        &rename.target,
        ClientRenameTarget::Group { old_path } if old_path == "bora-sync"
    ));

    assert!(state.handle_input_bytes(&[0x15]).actions.is_empty());
    let renamed = state.handle_input_bytes(b"sync\r");
    // ws_2 lives in `bora-sync/docs`, so it keeps its own `docs`
    // suffix under the new top-level name; ws_3 is untouched.
    assert_eq!(
        set_group_methods(&renamed),
        vec![
            ("ws_1".to_owned(), Some("sync".to_owned())),
            ("ws_2".to_owned(), Some("sync/docs".to_owned())),
        ]
    );
}

#[test]
fn group_rename_of_a_nested_group_keeps_its_parent_prefix() {
    let mut state = grouped_folders_state(false);
    let nested = workspace_rect(&state, "ws_2");
    right_click(&mut state, nested);
    click_menu_item(&mut state, &ClientContextMenuAction::RenameGroup);

    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_ref() else {
        panic!("group rename prompt");
    };
    // Only the last segment is offered for editing.
    assert_eq!(rename.input, "docs");

    assert!(state.handle_input_bytes(&[0x15]).actions.is_empty());
    let renamed = state.handle_input_bytes(b"manuals\r");
    assert_eq!(
        set_group_methods(&renamed),
        vec![("ws_2".to_owned(), Some("bora-sync/manuals".to_owned()))]
    );
}

#[test]
fn group_header_right_click_opens_the_group_menu_and_toggles_collapse() {
    let mut state = grouped_folders_state(false);
    let header = group_header_rect(&state, "vg:bora-sync");
    let opened = right_click(&mut state, header);
    assert!(opened.actions.is_empty());
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::GroupHeader { path, collapsed: false },
            ..
        })) if path == "bora-sync"
    ));
    assert_eq!(
        menu_labels(&state),
        vec![
            "Rename group…".to_owned(),
            "Collapse".to_owned(),
            "Ungroup all".to_owned(),
            "New workspace in group".to_owned(),
        ]
    );

    let collapsed = click_menu_item(&mut state, &ClientContextMenuAction::ToggleGroup);
    assert!(collapsed.repaint);
    assert!(state.collapsed_groups.contains("vg:bora-sync"));

    // Re-opening the header menu now offers the inverse label.
    state.compose(106, 30).expect("collapsed folders layout");
    let header = group_header_rect(&state, "vg:bora-sync");
    right_click(&mut state, header);
    assert_eq!(menu_labels(&state)[1], "Expand");
    click_menu_item(&mut state, &ClientContextMenuAction::ToggleGroup);
    assert!(!state.collapsed_groups.contains("vg:bora-sync"));
}

#[test]
fn group_header_rename_prompts_for_the_header_path() {
    let mut state = grouped_folders_state(false);
    let header = group_header_rect(&state, "vg:bora-sync");
    right_click(&mut state, header);
    click_menu_item(&mut state, &ClientContextMenuAction::RenameGroup);

    let Some(ClientShellOverlay::Rename(rename)) = state.overlay.as_ref() else {
        panic!("group rename prompt");
    };
    assert!(matches!(
        &rename.target,
        ClientRenameTarget::Group { old_path } if old_path == "bora-sync"
    ));
    let renamed = state.handle_input_bytes(&[0x15]);
    assert!(renamed.actions.is_empty());
    let renamed = state.handle_input_bytes(b"sync\r");
    assert_eq!(
        set_group_methods(&renamed),
        vec![
            ("ws_1".to_owned(), Some("sync".to_owned())),
            ("ws_2".to_owned(), Some("sync/docs".to_owned())),
        ]
    );
}

#[test]
fn group_header_ungroup_all_clears_the_group_and_its_nested_members() {
    let mut state = grouped_folders_state(false);
    let header = group_header_rect(&state, "vg:bora-sync");
    right_click(&mut state, header);

    let outcome = click_menu_item(&mut state, &ClientContextMenuAction::UngroupAll);
    assert_eq!(
        set_group_methods(&outcome),
        vec![("ws_1".to_owned(), None), ("ws_2".to_owned(), None)]
    );
}

#[test]
fn group_header_new_workspace_creates_inside_the_group() {
    let mut state = grouped_folders_state(false);
    let header = group_header_rect(&state, "vg:bora-sync");
    right_click(&mut state, header);

    let created = click_menu_item(&mut state, &ClientContextMenuAction::NewWorkspaceInGroup);
    let [ClientShellAction::Endpoint { request, .. }] = &created.actions[..] else {
        panic!(
            "new workspace should use endpoint API: {:?}",
            created.actions
        );
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceCreate(params)
            if params.group.as_deref() == Some("bora-sync") && params.focus
    ));
}

#[test]
fn group_header_new_workspace_prompt_carries_the_group_through_the_name_overlay() {
    let mut state = grouped_folders_state(true);
    let header = group_header_rect(&state, "vg:bora-sync");
    right_click(&mut state, header);

    let prompted = click_menu_item(&mut state, &ClientContextMenuAction::NewWorkspaceInGroup);
    assert!(prompted.actions.is_empty());
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::NewWorkspace { group: Some(group), .. },
            ..
        })) if group == "bora-sync"
    ));

    let created = state.handle_input_bytes(&[0x15]);
    assert!(created.actions.is_empty());
    let created = state.handle_input_bytes(b"notes\r");
    let [ClientShellAction::Endpoint { request, .. }] = &created.actions[..] else {
        panic!(
            "new workspace should use endpoint API: {:?}",
            created.actions
        );
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceCreate(params)
            if params.group.as_deref() == Some("bora-sync")
                && params.label.as_deref() == Some("notes")
    ));
}

/// "Move to group" must include a folder that exists only as a
/// synthesized ancestor of a nested path -- not only paths with a direct
/// exact member -- or a user can never move a workspace into a visible
/// parent folder (cubic review, ceo-bora#303 PR #32).
#[test]
fn workspace_context_menu_move_to_group_includes_a_synthesized_ancestor_folder() {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut projected = snapshot();
    // `bora-sync` never appears as an exact `visual_group` -- only as the
    // ancestor of `bora-sync/docs` -- so it is a header the sidebar
    // synthesizes, exactly the case the fix covers.
    projected.workspaces[0].visual_group = Some("bora-sync/docs".into());
    let mut loose = projected.workspaces[0].clone();
    loose.workspace_id = "ws_2".into();
    loose.number = 2;
    loose.label = "loose-space".into();
    loose.focused = false;
    loose.visual_group = None;
    projected.workspaces.push(loose);

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("folders layout");

    let loose_rect = workspace_rect(&state, "ws_2");
    right_click(&mut state, loose_rect);
    let labels = menu_labels(&state);
    assert!(
        labels.contains(&"→ bora-sync".to_owned()),
        "a folder with no direct member of its own must still be a move target: {labels:?}"
    );
    assert!(
        labels.contains(&"→ bora-sync/docs".to_owned()),
        "{labels:?}"
    );

    let moved = click_menu_item(
        &mut state,
        &ClientContextMenuAction::MoveToGroup("bora-sync".into()),
    );
    assert_eq!(
        set_group_methods(&moved),
        vec![("ws_2".to_owned(), Some("bora-sync".to_owned()))]
    );
}
