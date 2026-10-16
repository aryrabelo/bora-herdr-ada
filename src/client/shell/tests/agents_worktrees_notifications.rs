use super::*;

#[test]
fn mouse_hits_use_stable_workspace_tab_and_pane_ids() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");

    let workspace_down =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(workspace_down.actions.is_empty());
    let workspace =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(workspace.requests.is_empty());
    let [ClientShellAction::Endpoint { request, .. }] = &workspace.actions[..] else {
        panic!("workspace click should use the endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceFocus(target)
            if target.workspace_id == "ws_1"
    ));

    let pane = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 27,
        row: 1,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(pane.requests.is_empty());
    let [ClientShellAction::Endpoint { request, .. }] = &pane.actions[..] else {
        panic!("pane click should use the endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
    ));
}

#[test]
fn collapsed_workspace_jitter_remains_a_click() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.sidebar_collapsed = true;
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("collapsed sidebar");
    let workspace = state.hits.workspaces[0].rect;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: workspace.x,
        row: workspace.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: workspace.x + 1,
        row: workspace.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.chrome_drag.is_none());
    assert!(state.workspace_press.is_some());
    let release =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: workspace.x + 1,
            row: workspace.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target)
                    if target.workspace_id == "ws_1"
            )
    ));
}

#[test]
fn grouped_worktrees_render_parent_branch_and_indented_child() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut state = ClientShellState::new(config);
    let mut snapshot = snapshot();
    snapshot.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    snapshot.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_2".into(),
        active_tab_id: "tab_ws2".into(),
        new_workspace_cwd: "/repo/feature".into(),
        number: 2,
        label: "repo-feature".into(),
        custom_label: false,
        branch: Some("worktree/feature".into()),
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: true,
        }),
        visual_group: None,
        open_human_asks: 0,
        focused: false,
        agent_status: AgentStatus::Idle,
    });
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 20).expect("composed frame");
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
    assert!(text.contains("main"));
    assert!(text.contains("└─"));
    assert!(text.contains("feature"));

    let mut replacement = (**state.snapshot.as_ref().expect("snapshot")).clone();
    replacement.revision = 2;
    replacement.focused_workspace_id = Some("ws_2".into());
    replacement.workspaces[0].focused = false;
    replacement.workspaces[1].focused = true;
    replacement.workspaces[1].agent_status = AgentStatus::Blocked;
    let mut replacement_surface = surface();
    replacement_surface.projection_revision = 2;
    state.collapsed_groups.insert("repo".into());
    state.set_snapshot(Box::new(replacement));
    state.set_pane_surface(replacement_surface);
    let collapsed = state.compose(106, 20).expect("collapsed worktree group");
    let parent = state.hits.workspaces[0].rect;
    let status_cell = usize::from(parent.y) * usize::from(collapsed.width)
        + usize::from(parent.x.saturating_add(1));
    assert_eq!(
        collapsed.cells[status_cell].fg,
        crate::protocol::color_to_u32(state.config.palette.red)
    );

    let mut next = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextWorkspace),
        &mut next,
    );
    assert!(matches!(
        &next.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target)
                    if target.workspace_id == "ws_1"
            )
    ));
}

#[test]
fn duplicate_repo_parents_remain_visible_and_focusable_when_collapsed() {
    for linked_child in [false, true] {
        let mut projected = snapshot();
        projected.workspaces[0].agent_status = AgentStatus::Idle;
        projected.workspaces[0].worktree = Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: false,
        });
        let mut duplicate = projected.workspaces[0].clone();
        duplicate.workspace_id = "ws_duplicate".into();
        duplicate.label = "duplicate-repo".into();
        duplicate.focused = false;
        duplicate.agent_status = AgentStatus::Blocked;
        projected.workspaces.push(duplicate);
        if linked_child {
            let mut child = projected.workspaces[0].clone();
            child.workspace_id = "ws_child".into();
            child.focused = false;
            child.worktree.as_mut().unwrap().is_linked_worktree = true;
            projected.workspaces.push(child);
        }

        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(projected));
        state.set_pane_surface(surface());
        for collapsed in [false, true] {
            if collapsed {
                state.collapsed_groups.insert("repo".into());
            }
            let frame = state
                .compose(106, 24)
                .expect("duplicate repository parents");
            for workspace_id in ["ws_1", "ws_duplicate"] {
                let hit = state
                    .hits
                    .workspaces
                    .iter()
                    .find(|hit| hit.workspace_id == workspace_id)
                    .expect("repository parent remains visible");
                assert!(!hit.indented);
                assert_eq!(hit.group_toggle.is_some(), linked_child);
            }
            let parent = state.hits.workspaces[0].rect;
            let status_cell =
                usize::from(parent.y) * usize::from(frame.width) + usize::from(parent.x + 1);
            assert_ne!(
                frame.cells[status_cell].fg,
                crate::protocol::color_to_u32(state.config.palette.red)
            );
            assert_eq!(
                state
                    .hits
                    .workspaces
                    .iter()
                    .any(|hit| hit.workspace_id == "ws_child" && hit.indented),
                linked_child && !collapsed
            );
            let duplicate = state.hits.workspaces[1].rect;
            state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: duplicate.x + 2,
                row: duplicate.y,
                modifiers: KeyModifiers::empty(),
            })]);
            let click = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: duplicate.x + 2,
                row: duplicate.y,
                modifiers: KeyModifiers::empty(),
            })]);
            assert!(matches!(click.actions.as_slice(),
                [ClientShellAction::Endpoint { request, .. }]
                    if matches!(&request.method, crate::api::schema::Method::WorkspaceFocus(target)
                        if target.workspace_id == "ws_duplicate")));
        }
        let original = state.snapshot.as_deref().unwrap().clone();
        for confirm_close in [false, true] {
            let mut restored = original.clone();
            restored.revision = state.snapshot.as_deref().unwrap().revision + 1;
            state.set_snapshot(Box::new(restored));
            state.config.confirm_close = confirm_close;
            state.open_workspace_context_menu("ws_duplicate".into(), 0, 0);
            let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
                panic!("repository workspace context menu");
            };
            assert_eq!(menu.items()[1].label, "Close");
            assert_eq!(
                menu.items()
                    .iter()
                    .any(|item| matches!(item.action, ClientContextMenuAction::ToggleGroup)),
                linked_child
            );
            if linked_child {
                let mut replacement = original.clone();
                replacement.revision = state.snapshot.as_deref().unwrap().revision + 1;
                replacement
                    .workspaces
                    .retain(|workspace| workspace.workspace_id != "ws_1");
                state.set_snapshot(Box::new(replacement));
            }
            let mut close = ClientShellInput::default();
            state.activate_context_menu_item(1, &mut close);
            if confirm_close {
                assert!(close.actions.is_empty());
                assert!(matches!(state.overlay.as_ref(),
                    Some(ClientShellOverlay::ConfirmClose(confirm))
                        if confirm.title == "Close workspace?" && !confirm.close_group));
                close = state.handle_input_bytes(b"\r");
            }
            assert!(matches!(close.actions.as_slice(),
                [ClientShellAction::Endpoint { request, .. }]
                    if matches!(&request.method, crate::api::schema::Method::WorkspaceClose(params)
                        if params.workspace_id == "ws_duplicate" && !params.close_group)));
        }
    }
}

#[test]
fn workspace_click_waits_for_release_and_drag_reorders_by_stable_id() {
    let mut projected = snapshot();
    for index in 2..=3 {
        let mut workspace = projected.workspaces[0].clone();
        workspace.workspace_id = format!("ws_{index}");
        workspace.number = index;
        workspace.label = format!("workspace-{index}");
        workspace.focused = false;
        projected.workspaces.push(workspace);
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("three workspaces");
    let first = state.hits.workspaces[0].rect;
    let third = state.hits.workspaces[2].rect;

    let down = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: first.x + 2,
        row: first.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(down.actions.is_empty());
    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: third.x + 2,
        row: third.bottom(),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(drag.repaint);
    assert!(matches!(
        &state.chrome_drag,
        Some(ClientChromeDrag::Workspace {
            source_workspace_id,
            target: Some((None, _)),
            ..
        }) if source_workspace_id == "ws_1"
    ));
    let frame = state.compose(106, 24).expect("workspace drop indicator");
    assert!(frame
        .cells
        .chunks(frame.width as usize)
        .any(|row| row.iter().take(20).any(|cell| cell.symbol == "─")));

    let release =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: third.x + 2,
            row: third.bottom(),
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceMove(params)
                    if params.workspace_id == "ws_1" && params.insert_index == 3
            )
    ));

    state.compose(106, 24).expect("workspaces after drag");
    let second = state.hits.workspaces[1].rect;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: second.x + 2,
        row: second.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: second.x + 2,
        row: second.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &click.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target)
                    if target.workspace_id == "ws_2"
            )
    ));
}

#[test]
fn duplicate_repo_parent_drag_does_not_target_its_own_move_block() {
    let mut projected = snapshot();
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    let mut other = projected.workspaces[0].clone();
    other.workspace_id = "ws_other".into();
    other.worktree = None;
    other.focused = false;
    let mut duplicate = projected.workspaces[0].clone();
    duplicate.workspace_id = "ws_duplicate".into();
    duplicate.focused = false;
    projected.workspaces.extend([other, duplicate]);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
        .compose(106, 24)
        .expect("separated repository parents");
    let source = state.hits.workspaces[0].rect;
    let target = state.hits.workspaces[2].rect;
    for (kind, rect, row) in [
        (MouseEventKind::Down(MouseButton::Left), source, source.y),
        (
            MouseEventKind::Drag(MouseButton::Left),
            target,
            target.y.saturating_sub(1),
        ),
    ] {
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind,
            column: rect.x + 2,
            row,
            modifiers: KeyModifiers::empty(),
        })]);
    }
    assert!(
        matches!(state.chrome_drag.as_ref(), Some(ClientChromeDrag::Workspace {
        target: Some((Some(workspace_id), _)), ..
    }) if workspace_id == "ws_duplicate")
    );
    let drop = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: target.x + 2,
        row: target.y.saturating_sub(1),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(drop.actions.is_empty());
}

#[test]
fn workspace_drag_moves_parent_worktree_as_one_block_and_rejects_child() {
    let mut projected = snapshot();
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    let mut child = projected.workspaces[0].clone();
    child.workspace_id = "ws_child".into();
    child.number = 2;
    child.label = "feature".into();
    child.focused = false;
    child.worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    let mut other = projected.workspaces[0].clone();
    other.workspace_id = "ws_other".into();
    other.number = 3;
    other.label = "other".into();
    other.focused = false;
    other.worktree = None;
    projected.workspaces.extend([child, other]);

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("worktree workspaces");
    assert!(state.hits.workspaces[1].indented);
    let parent = state.hits.workspaces[0].rect;
    let child = state.hits.workspaces[1].rect;
    let other = state.hits.workspaces[2].rect;

    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: parent.x + 2,
        row: parent.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: other.x + 2,
        row: other.bottom(),
        modifiers: KeyModifiers::empty(),
    })]);
    let moved = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: other.x + 2,
        row: other.bottom(),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &moved.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceMoveBlock(params)
                    if params.workspace_ids == ["ws_1", "ws_child"]
                        && params.before_workspace_id.is_none()
            )
    ));

    state.compose(106, 24).expect("worktree child");
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: child.x + 2,
        row: child.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let dragging_child =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: other.x + 2,
            row: other.bottom(),
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(dragging_child.actions.is_empty());
    assert!(state.chrome_drag.is_none());

    let mut projected = state.snapshot.as_deref().unwrap().clone();
    let mut duplicate = projected.workspaces[0].clone();
    duplicate.workspace_id = "ws_duplicate".into();
    duplicate.focused = false;
    projected.workspaces.push(duplicate);
    state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
        .compose(106, 24)
        .expect("repository block with duplicate root");
    let duplicate = state.hits.workspaces[1].rect;
    let other = state.hits.workspaces[3].rect;
    for (source, target_row, moving_duplicate) in [
        (other, duplicate.y.saturating_sub(1), false),
        (duplicate, other.bottom(), true),
    ] {
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: source.x + 2,
            row: source.y,
            modifiers: KeyModifiers::empty(),
        })]);
        state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: source.x + 2,
            row: target_row,
            modifiers: KeyModifiers::empty(),
        })]);
        let moved = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: source.x + 2,
            row: target_row,
            modifiers: KeyModifiers::empty(),
        })]);
        if moving_duplicate {
            assert!(matches!(moved.actions.as_slice(),
                [ClientShellAction::Endpoint { request, .. }]
                    if matches!(&request.method, crate::api::schema::Method::WorkspaceMoveBlock(params)
                        if params.workspace_ids == ["ws_duplicate", "ws_1", "ws_child"] && params.before_workspace_id.is_none())));
        } else {
            assert!(matches!(moved.actions.as_slice(),
                [ClientShellAction::Endpoint { request, .. }]
                    if matches!(&request.method, crate::api::schema::Method::WorkspaceMove(params)
                        if params.workspace_id == "ws_other" && params.insert_index == 0)));
        }
    }
}

#[test]
fn pane_cycle_last_and_agent_actions_resolve_to_stable_pane_ids() {
    let mut initial = snapshot();
    let mut second = initial.panes[0].clone();
    second.pane_id = "pane_2".into();
    second.focused = false;
    initial.panes.push(second);
    initial.agents = vec![
        ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("first".into()),
            display_agent: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
        },
        ClientShellAgent {
            pane_id: "pane_2".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("second".into()),
            display_agent: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 2,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        },
    ];
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(initial.clone()));

    let mut cycle = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CyclePaneNext),
        &mut cycle,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &cycle.actions[..] else {
        panic!("pane cycle should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
    ));

    let mut replacement = initial;
    replacement.revision = 2;
    replacement.focused_pane_id = Some("pane_2".into());
    replacement.panes[0].focused = false;
    replacement.panes[1].focused = true;
    state.set_snapshot(Box::new(replacement));
    let mut last = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::LastPane),
        &mut last,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &last.actions[..] else {
        panic!("last pane should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
    ));

    let mut agent = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::FocusAgent(1)),
        &mut agent,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &agent.actions[..] else {
        panic!("agent focus should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
    ));
}

#[test]
fn agent_sidebar_honors_priority_symbols_tokens_and_stable_hits() {
    let mut projected = snapshot();
    let mut second_pane = projected.panes[0].clone();
    second_pane.pane_id = "pane_2".into();
    second_pane.focused = false;
    projected.panes.push(second_pane);
    projected.agents = vec![
        ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("pi one".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: Some("first title".into()),
            terminal_title_stripped: Some("first".into()),
            agent_status: AgentStatus::Done,
            state_change_seq: 10,
            state_labels: Vec::new(),
            tokens: vec![("summary".into(), "review complete".into())],
            focused: true,
        },
        ClientShellAgent {
            pane_id: "pane_2".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("pi two".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: Some("second title".into()),
            terminal_title_stripped: Some("second".into()),
            agent_status: AgentStatus::Blocked,
            state_change_seq: 20,
            state_labels: vec![("blocked".into(), "needs input".into())],
            tokens: vec![("summary".into(), "waiting for Can".into())],
            focused: false,
        },
    ];
    let mut config = Config::default();
    config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    config.ui.sidebar.agents.rows = vec![vec![crate::config::AgentSidebarToken::Agent]];
    config.ui.sidebar.agents.rows_by_agent.insert(
        "pi".into(),
        vec![
            vec![
                crate::config::AgentSidebarToken::StateIcon,
                crate::config::AgentSidebarToken::StateText,
            ],
            vec![
                crate::config::AgentSidebarToken::Agent,
                crate::config::AgentSidebarToken::Custom("summary".into()),
            ],
        ],
    );
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 30).expect("agent sidebar frame");
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
    assert!(text.contains("× needs input"), "frame: {text}");
    assert!(text.contains("pi two"), "frame: {text}");
    assert!(text.contains("waiting for"), "frame: {text}");
    assert_eq!(
        state
            .hits
            .agents
            .first()
            .map(|(_, pane_id)| pane_id.as_str()),
        Some("pane_2")
    );

    let first = state.hits.agents[0].0;
    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: first.x,
        row: first.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let [ClientShellAction::Endpoint { request, .. }] = &click.actions[..] else {
        panic!("agent row should focus through endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
    ));

    state.compose(106, 10).expect("short agent sidebar frame");
    assert_eq!(
        state
            .hits
            .agents
            .first()
            .map(|(_, pane_id)| pane_id.as_str()),
        Some("pane_2")
    );
    let body = state.hits.agent_body;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: body.x,
        row: body.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state
        .compose(106, 10)
        .expect("scrolled agent sidebar frame");
    assert_eq!(
        state
            .hits
            .agents
            .first()
            .map(|(_, pane_id)| pane_id.as_str()),
        Some("pane_1")
    );

    state.sidebar_collapsed = true;
    let compact = state.compose(106, 30).expect("compact agent sidebar frame");
    let blocked = state
        .hits
        .agents
        .iter()
        .find(|(_, pane_id)| pane_id == "pane_2")
        .expect("blocked compact agent")
        .0;
    let row_start = blocked.y as usize * compact.width as usize + blocked.x as usize;
    assert_ne!(compact.cells[row_start].fg, compact.cells[row_start + 2].fg);
    assert_eq!(compact.cells[row_start].bg, compact.cells[row_start + 2].bg);
}

#[test]
fn muted_agent_sidebar_rows_do_not_stack_terminal_faint() {
    let mut projected = snapshot();
    projected.tabs[0].label = "second".into();
    projected.tabs[0].custom_label = true;
    projected.agents = vec![ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("reviewer".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    }];
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 30).expect("agent sidebar frame");
    let row = state.hits.agents.first().expect("agent row hit").0;
    let buffer = frame.to_ratatui_buffer().expect("agent sidebar buffer");

    for (label, needle) in [("tab", "second"), ("agent", "reviewer"), ("separator", "·")] {
        let (x, y) = cell_symbol_position(&frame, row, needle);
        let cell = buffer.cell((x, y)).expect("muted sidebar cell");
        assert!(
            !cell.modifier.contains(Modifier::DIM),
            "{label} cell at ({x},{y}) should not stack terminal faint: {cell:?}"
        );
    }
}

#[test]
fn idle_tier_colors_a_fresh_and_a_stale_agent_name_from_the_rendered_buffer() {
    let mut projected = snapshot();
    projected.panes[0].idle_seconds = Some(0);
    let mut stale_pane = projected.panes[0].clone();
    stale_pane.pane_id = "pane_2".into();
    stale_pane.focused = false;
    stale_pane.idle_seconds = Some(7_200);
    projected.panes.push(stale_pane);
    projected.agents = vec![
        ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("fresh-agent".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        },
        ClientShellAgent {
            pane_id: "pane_2".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("stale-agent".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 2,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        },
    ];
    let config = Config::default();
    let shell_config = ClientShellConfig::from_config(&config);
    let expected_fresh = shell_config.palette.teal;
    let expected_stale = shell_config.palette.overlay0;
    let mut state = ClientShellState::new(shell_config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 30).expect("agent sidebar frame");
    let buffer = frame.to_ratatui_buffer().expect("agent sidebar buffer");
    let fresh_row = state
        .hits
        .agents
        .iter()
        .find(|(_, pane_id)| pane_id == "pane_1")
        .expect("fresh agent hit")
        .0;
    let stale_row = state
        .hits
        .agents
        .iter()
        .find(|(_, pane_id)| pane_id == "pane_2")
        .expect("stale agent hit")
        .0;
    let (fresh_x, fresh_y) = cell_symbol_position(&frame, fresh_row, "fresh-agent");
    let (stale_x, stale_y) = cell_symbol_position(&frame, stale_row, "stale-agent");
    let fresh_cell = buffer.cell((fresh_x, fresh_y)).expect("fresh name cell");
    let stale_cell = buffer.cell((stale_x, stale_y)).expect("stale name cell");

    assert_eq!(
        fresh_cell.fg, expected_fresh,
        "fresh agent name should use the Done/teal color: {fresh_cell:?}"
    );
    assert_eq!(
        stale_cell.fg, expected_stale,
        "stale agent name should use overlay0: {stale_cell:?}"
    );
}

/// cubic P2 on aryrabelo/bora-herdr-ada#52: the 16-color `terminal` theme
/// resolves the palette to named colors, and the tier recolor used to drop
/// any non-RGB color, so every tier fell back to the untiered `overlay0`
/// (`Gray`). The named colors must reach the buffer. Under this palette
/// `subtext0` and `overlay0` are both `Gray`, so an unfocused Plain row reads
/// the same as a Stale one; only Fresh (`Cyan`) stands apart.
#[test]
fn idle_tier_colors_names_under_the_terminal_palette() {
    use ratatui::style::Color;

    let mut projected = snapshot();
    let mut agents = Vec::new();
    projected.panes.clear();
    for (index, (name, idle)) in [
        ("fresh-agent", 0),
        ("plain-agent", 1_800),
        ("stale-agent", 7_200),
    ]
    .into_iter()
    .enumerate()
    {
        let mut pane = snapshot().panes[0].clone();
        pane.pane_id = format!("pane_{}", index + 1);
        pane.focused = false;
        pane.idle_seconds = Some(idle);
        projected.panes.push(pane);
        agents.push(ClientShellAgent {
            pane_id: format!("pane_{}", index + 1),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some(name.into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: index as u64 + 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        });
    }
    projected.agents = agents;
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.palette = Palette::terminal();
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());

    let frame = state.compose(106, 30).expect("agent sidebar frame");
    let buffer = frame.to_ratatui_buffer().expect("agent sidebar buffer");
    let name_fg = |pane_id: &str, name: &str| {
        let row = state
            .hits
            .agents
            .iter()
            .find(|(_, hit)| hit == pane_id)
            .unwrap_or_else(|| panic!("{name} hit"))
            .0;
        let (x, y) = cell_symbol_position(&frame, row, name);
        buffer.cell((x, y)).expect("agent name cell").fg
    };

    assert_eq!(name_fg("pane_1", "fresh-agent"), Color::Cyan);
    assert_eq!(name_fg("pane_2", "plain-agent"), Color::Gray);
    assert_eq!(name_fg("pane_3", "stale-agent"), Color::Gray);
}

/// ceo-bora#352 live-smoke defect: the server stamps `idle_seconds` only when
/// it builds a snapshot, so on a quiet screen a Fresh row stayed teal until
/// some unrelated snapshot arrived. Here no second snapshot is ever sent;
/// only the client's poll tick advances time, and the row must still cross
/// Fresh -> Plain and ask for the repaint that draws it.
#[test]
fn idle_tier_row_crosses_fresh_to_plain_without_a_new_snapshot() {
    let mut projected = snapshot();
    projected.panes[0].idle_seconds = Some(899);
    projected.agents = vec![ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("ageing-agent".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Idle,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    }];
    let shell_config = ClientShellConfig::from_config(&Config::default());
    let fresh = shell_config.palette.teal;
    let plain = shell_config.palette.subtext0;
    let mut state = ClientShellState::new(shell_config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let name_fg = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("agent sidebar frame");
        let row = state.hits.agents.first().expect("agent row hit").0;
        let (x, y) = cell_symbol_position(&frame, row, "ageing-agent");
        frame
            .to_ratatui_buffer()
            .expect("agent sidebar buffer")
            .cell((x, y))
            .expect("agent name cell")
            .fg
    };
    assert_eq!(name_fg(&mut state), fresh);

    let start = std::time::Instant::now();
    assert!(
        !state.tick_idle_ageing(start),
        "the first tick only takes the baseline"
    );
    assert_eq!(name_fg(&mut state), fresh);
    assert!(
        state.tick_idle_ageing(start + std::time::Duration::from_secs(1)),
        "crossing the 15-minute fresh edge must request a repaint"
    );
    assert_eq!(name_fg(&mut state), plain);
}

/// ceo-bora#353 fixture, in `snapshot.agents` order: `ws_1` runs a stale
/// idle agent ahead of a working one and a fresh idle one; `ws_2` runs a
/// stale done agent ahead of a fresh idle one (the client projects that
/// first-seen Done to Idle, so it ranks by `state_change_seq` like the
/// other idle rows). Default edges: stale at 120 minutes (7200 s), fresh
/// under 15.
fn stale_row_snapshot() -> ClientShellSnapshot {
    two_space_agent_snapshot(&[
        (
            "pane_1",
            "ws_1",
            "stale-alpha",
            AgentStatus::Idle,
            5,
            Some(7_200),
        ),
        (
            "pane_2",
            "ws_1",
            "busy-bravo",
            AgentStatus::Working,
            4,
            None,
        ),
        (
            "pane_3",
            "ws_1",
            "calm-charlie",
            AgentStatus::Idle,
            3,
            Some(60),
        ),
        (
            "pane_4",
            "ws_2",
            "stale-delta",
            AgentStatus::Done,
            2,
            Some(9_000),
        ),
        (
            "pane_5",
            "ws_2",
            "fresh-echo",
            AgentStatus::Idle,
            1,
            Some(0),
        ),
    ])
}

/// One agent fixture row: pane id, workspace (`ws_1` or `ws_2`), agent
/// name, status, `state_change_seq`, and the pane's `idle_seconds`.
type AgentOrderRow<'a> = (&'a str, &'a str, &'a str, AgentStatus, u64, Option<u64>);

/// A snapshot with two spaces (`ws_1`/`tab_1`, `ws_2`/`tab_2`) whose
/// `agents` and `panes` are `rows`, in that order.
fn two_space_agent_snapshot(rows: &[AgentOrderRow<'_>]) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut second_workspace = projected.workspaces[0].clone();
    second_workspace.workspace_id = "ws_2".into();
    second_workspace.active_tab_id = "tab_2".into();
    second_workspace.number = 2;
    second_workspace.label = "second-space".into();
    second_workspace.focused = false;
    projected.workspaces.push(second_workspace);
    let mut second_tab = projected.tabs[0].clone();
    second_tab.tab_id = "tab_2".into();
    second_tab.workspace_id = "ws_2".into();
    second_tab.focused = false;
    projected.tabs.push(second_tab);
    let tab_of = |workspace_id: &str| {
        if workspace_id == "ws_1" {
            "tab_1"
        } else {
            "tab_2"
        }
    };
    let template = projected.panes[0].clone();
    projected.panes = rows
        .iter()
        .map(
            |&(pane_id, workspace_id, _, _, _, idle_seconds)| ClientShellPane {
                pane_id: pane_id.into(),
                workspace_id: workspace_id.into(),
                tab_id: tab_of(workspace_id).into(),
                focused: false,
                idle_seconds,
                ..template.clone()
            },
        )
        .collect();
    projected.agents = rows
        .iter()
        .map(
            |&(pane_id, workspace_id, name, agent_status, state_change_seq, _)| ClientShellAgent {
                pane_id: pane_id.into(),
                workspace_id: workspace_id.into(),
                tab_id: tab_of(workspace_id).into(),
                name: Some(name.into()),
                display_agent: None,
                agent: Some("pi".into()),
                title: None,
                terminal_title: None,
                terminal_title_stripped: None,
                agent_status,
                state_change_seq,
                state_labels: Vec::new(),
                tokens: Vec::new(),
                focused: false,
            },
        )
        .collect();
    projected
}

const STALE_ROW_NAMES: [&str; 5] = [
    "stale-alpha",
    "busy-bravo",
    "calm-charlie",
    "stale-delta",
    "fresh-echo",
];

fn stale_row_state(config: &Config, projected: ClientShellSnapshot) -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state
}

/// Agent names top to bottom as the panel drew them, read off the frame.
fn stale_row_drawn_order<'a>(state: &mut ClientShellState, names: &[&'a str]) -> Vec<&'a str> {
    let frame = state.compose(106, 60).expect("agent panel frame");
    let body = state.hits.agent_body;
    let mut drawn = names
        .iter()
        .map(|name| {
            let (x, y) = cell_symbol_position(&frame, body, name);
            ((y, x), *name)
        })
        .collect::<Vec<_>>();
    drawn.sort_by_key(|(position, _)| *position);
    drawn.into_iter().map(|(_, name)| name).collect()
}

/// How many cells of `pane_id`'s drawn row carry `Modifier::DIM`, and how
/// many cells the row has.
fn stale_row_dim_cells(state: &mut ClientShellState, pane_id: &str) -> (usize, usize) {
    let frame = state.compose(106, 60).expect("agent panel frame");
    let buffer = frame.to_ratatui_buffer().expect("agent panel buffer");
    let rect = state
        .hits
        .agents
        .iter()
        .find(|(_, hit)| hit == pane_id)
        .unwrap_or_else(|| panic!("{pane_id} row hit"))
        .0;
    let cells = (rect.y..rect.bottom())
        .flat_map(|y| (rect.x..rect.right()).map(move |x| (x, y)))
        .collect::<Vec<_>>();
    let dim = cells
        .iter()
        .filter(|&&position| {
            buffer
                .cell(position)
                .expect("agent row cell")
                .modifier
                .contains(Modifier::DIM)
        })
        .count();
    (dim, cells.len())
}

/// The keyboard's `FocusAgent(n)` targets, for every drawn row, as names.
fn stale_row_keyboard_order(state: &mut ClientShellState, names: &[&str]) -> Vec<String> {
    let agents = state.snapshot.as_deref().expect("snapshot").agents.clone();
    (0..names.len())
        .map(|index| {
            let Some(crate::api::schema::Method::PaneFocus(target)) =
                state.endpoint_method_for_action(crate::input::KeybindAction::FocusAgent(index))
            else {
                panic!("FocusAgent({index}) must focus a pane");
            };
            agents
                .iter()
                .find(|agent| agent.pane_id == target.pane_id)
                .and_then(|agent| agent.name.clone())
                .expect("focused agent name")
        })
        .collect()
}

#[test]
fn stale_row_is_dim_on_every_cell_and_a_live_row_is_not() {
    let mut state = stale_row_state(&Config::default(), stale_row_snapshot());
    let frame = state.compose(106, 60).expect("agent panel frame");
    let stale = state
        .hits
        .agents
        .iter()
        .find(|(_, pane_id)| pane_id == "pane_1")
        .expect("stale row hit")
        .0;
    // Fixture guard: the row whose cells are checked holds the stale
    // agent's name on its second line and its state icon on the first.
    assert_eq!(
        cell_symbol_position(&frame, stale, "stale-alpha").1,
        stale.y + 1
    );
    assert!(stale.height >= 2, "stale row rect: {stale:?}");

    let (dim, cells) = stale_row_dim_cells(&mut state, "pane_1");
    assert_eq!(dim, cells, "every cell of the stale idle row is dim");
    let (dim, cells) = stale_row_dim_cells(&mut state, "pane_4");
    assert_eq!(dim, cells, "every cell of the stale done row is dim");
    for live in ["pane_2", "pane_3", "pane_5"] {
        assert_eq!(
            stale_row_dim_cells(&mut state, live).0,
            0,
            "{live} is not stale and carries no dim cell"
        );
    }
}

#[test]
fn stale_row_sinks_to_the_end_of_its_workspace_run_in_spaces_order() {
    let mut state = stale_row_state(&Config::default(), stale_row_snapshot());
    let drawn = stale_row_drawn_order(&mut state, &STALE_ROW_NAMES);
    assert_eq!(
        drawn,
        [
            "busy-bravo",
            "calm-charlie",
            "stale-alpha",
            "fresh-echo",
            "stale-delta"
        ]
    );
    assert_eq!(
        stale_row_keyboard_order(&mut state, &STALE_ROW_NAMES),
        drawn
    );
}

#[test]
fn stale_row_sinks_below_every_live_row_in_priority_order() {
    let mut config = Config::default();
    config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    let mut state = stale_row_state(&config, stale_row_snapshot());
    let drawn = stale_row_drawn_order(&mut state, &STALE_ROW_NAMES);
    // Without the sink the idle rows rank by `state_change_seq`, so
    // `stale-alpha` (5) would lead `calm-charlie` (3) and `fresh-echo` (1)
    // and `stale-delta` (2) would sit above `fresh-echo`. Both stale rows
    // drop below every live row and keep their order between themselves.
    assert_eq!(
        drawn,
        [
            "busy-bravo",
            "calm-charlie",
            "fresh-echo",
            "stale-alpha",
            "stale-delta"
        ]
    );
    assert_eq!(
        stale_row_keyboard_order(&mut state, &STALE_ROW_NAMES),
        drawn
    );
}

#[test]
fn stale_row_keeps_the_agent_view_order_while_dimmed() {
    let mut projected = stale_row_snapshot();
    projected.agent_view_label = Some("plugin view".into());
    projected.agent_order = ["pane_1", "pane_4", "pane_2", "pane_5", "pane_3"]
        .map(String::from)
        .to_vec();
    let mut state = stale_row_state(&Config::default(), projected);
    let drawn = stale_row_drawn_order(&mut state, &STALE_ROW_NAMES);
    assert_eq!(
        drawn,
        [
            "stale-alpha",
            "stale-delta",
            "busy-bravo",
            "fresh-echo",
            "calm-charlie"
        ]
    );
    assert_eq!(
        stale_row_keyboard_order(&mut state, &STALE_ROW_NAMES),
        drawn
    );
    for stale in ["pane_1", "pane_4"] {
        let (dim, cells) = stale_row_dim_cells(&mut state, stale);
        assert_eq!(dim, cells, "{stale} stays dim under the view");
    }
}

#[test]
fn stale_row_dim_and_sink_follow_dim_stale_agents_on_load_and_live_reload() {
    let unsorted = [
        "stale-alpha",
        "busy-bravo",
        "calm-charlie",
        "stale-delta",
        "fresh-echo",
    ];
    let mut off = Config::default();
    off.ui.dim_stale_agents = false;

    let mut state = stale_row_state(&off, stale_row_snapshot());
    assert_eq!(
        stale_row_drawn_order(&mut state, &STALE_ROW_NAMES),
        unsorted
    );
    assert_eq!(stale_row_dim_cells(&mut state, "pane_1").0, 0);

    state.config.apply_live_config(&Config::default(), &[], &[]);
    assert_ne!(
        stale_row_drawn_order(&mut state, &STALE_ROW_NAMES),
        unsorted
    );
    let (dim, cells) = stale_row_dim_cells(&mut state, "pane_1");
    assert_eq!(dim, cells);

    state.config.apply_live_config(&off, &[], &[]);
    assert_eq!(
        stale_row_drawn_order(&mut state, &STALE_ROW_NAMES),
        unsorted
    );
    assert_eq!(stale_row_dim_cells(&mut state, "pane_1").0, 0);
}

/// The #352 quiet-screen lesson applied to the order: no second snapshot
/// is ever sent, only the poll tick moves time, and the row must cross
/// into stale, turn dim AND move -- in the panel and for the keyboard.
#[test]
fn stale_row_turns_dim_and_sinks_without_a_new_snapshot() {
    let mut projected = stale_row_snapshot();
    projected.agents.truncate(2);
    projected.panes.truncate(2);
    projected.panes[0].idle_seconds = Some(7_199);
    let names = ["stale-alpha", "busy-bravo"];
    let mut state = stale_row_state(&Config::default(), projected);
    assert_eq!(stale_row_drawn_order(&mut state, &names), names);
    assert_eq!(stale_row_dim_cells(&mut state, "pane_1").0, 0);

    let start = std::time::Instant::now();
    assert!(
        !state.tick_idle_ageing(start),
        "the first tick only takes the baseline"
    );
    assert_eq!(stale_row_drawn_order(&mut state, &names), names);
    assert!(
        state.tick_idle_ageing(start + std::time::Duration::from_secs(1)),
        "crossing the 120-minute stale edge must request a repaint"
    );
    assert_eq!(
        stale_row_drawn_order(&mut state, &names),
        ["busy-bravo", "stale-alpha"]
    );
    assert_eq!(
        stale_row_keyboard_order(&mut state, &names),
        ["busy-bravo", "stale-alpha"]
    );
    let (dim, cells) = stale_row_dim_cells(&mut state, "pane_1");
    assert_eq!(dim, cells);
}

/// The collapsed sidebar's agent rail draws the same sunk order and must
/// dim the stale rows too (cubic review on ceo-bora#353's PR).
#[test]
fn stale_row_is_dim_on_every_cell_of_the_collapsed_sidebar_rail() {
    let mut state = stale_row_state(&Config::default(), stale_row_snapshot());
    state.sidebar_collapsed = true;
    for stale in ["pane_1", "pane_4"] {
        let (dim, cells) = stale_row_dim_cells(&mut state, stale);
        // Fixture guard: the collapsed rail's one-line row, not the
        // expanded panel's two-line one.
        let rect = state
            .hits
            .agents
            .iter()
            .find(|(_, pane_id)| pane_id == stale)
            .expect("rail row hit")
            .0;
        assert_eq!(rect.height, 1, "{stale} rail row: {rect:?}");
        assert!(cells > 0, "{stale} rail row has cells");
        assert_eq!(dim, cells, "every cell of collapsed {stale} is dim");
    }
    for live in ["pane_2", "pane_3", "pane_5"] {
        assert_eq!(stale_row_dim_cells(&mut state, live).0, 0, "{live} is live");
    }

    let mut off = Config::default();
    off.ui.dim_stale_agents = false;
    state.config.apply_live_config(&off, &[], &[]);
    assert_eq!(stale_row_dim_cells(&mut state, "pane_1").0, 0);
}

#[test]
fn workspace_state_text_does_not_stack_terminal_faint() {
    use crate::config::SpaceSidebarToken;

    let mut config = Config::default();
    config.ui.sidebar.spaces.rows = vec![
        vec![SpaceSidebarToken::StateIcon, SpaceSidebarToken::Workspace],
        vec![SpaceSidebarToken::StateText],
    ];
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 30).expect("workspace sidebar frame");
    let rect = state.hits.workspaces.first().expect("workspace hit").rect;
    let buffer = frame.to_ratatui_buffer().expect("workspace sidebar buffer");
    let (x, y) = cell_symbol_position(&frame, rect, "idle");
    let cell = buffer.cell((x, y)).expect("workspace state text cell");
    assert!(
        !cell.modifier.contains(Modifier::DIM),
        "workspace state text at ({x},{y}) should not stack terminal faint: {cell:?}"
    );
}

#[test]
fn active_agent_view_controls_sidebar_order_and_focus_indices() {
    let mut projected = snapshot();
    let mut second_pane = projected.panes[0].clone();
    second_pane.pane_id = "pane_2".into();
    second_pane.focused = false;
    projected.panes.push(second_pane.clone());
    let mut third_pane = second_pane;
    third_pane.pane_id = "pane_3".into();
    projected.panes.push(third_pane);
    projected.agents = vec![
        ClientShellAgent {
            pane_id: "pane_1".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("first".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 1,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: true,
        },
        ClientShellAgent {
            pane_id: "pane_2".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("second".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Blocked,
            state_change_seq: 2,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        },
        ClientShellAgent {
            pane_id: "pane_3".into(),
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some("third".into()),
            display_agent: None,
            agent: Some("pi".into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: AgentStatus::Idle,
            state_change_seq: 3,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        },
    ];
    projected.agent_view_label = Some("review".into());
    projected.agent_order = vec!["pane_2".into(), "pane_3".into()];
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("filtered agent sidebar");
    assert_eq!(
        state
            .hits
            .agents
            .iter()
            .map(|(_, pane_id)| pane_id.as_str())
            .collect::<Vec<_>>(),
        vec!["pane_2", "pane_3"]
    );
    assert_eq!(state.hits.agent_sort_toggle, Rect::default());

    let mut focus = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::FocusAgent(0)),
        &mut focus,
    );
    assert!(matches!(
        &focus.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
            )
    ));

    let mut next = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextAgent),
        &mut next,
    );
    assert!(matches!(
        &next.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_2"
            )
    ));
}

#[test]
fn agent_sort_toggle_is_client_local_and_persists_per_endpoint() {
    let path = std::env::temp_dir().join(format!(
        "herdr-shell-agent-sort-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let mut projected = snapshot();
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: Some("pi".into()),
        display_agent: None,
        agent: Some("pi".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: true,
    });
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("agent sidebar frame");
    let toggle = state.hits.agent_sort_toggle;

    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: toggle.x,
        row: toggle.y,
        modifiers: KeyModifiers::empty(),
    })]);

    assert_eq!(
        state.config.agent_panel_sort,
        crate::config::AgentPanelSortConfig::Priority
    );
    assert!(click.actions.is_empty());
    let reloaded_config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let reloaded = ClientShellState::new(reloaded_config);
    assert_eq!(
        reloaded.config.agent_panel_sort,
        crate::config::AgentPanelSortConfig::Priority
    );
    assert!(reloaded.agent_panel_sort_manual);
    std::fs::remove_file(path).expect("remove agent sort preferences");
}

/// ceo-bora#354 `recent` fixture, in `snapshot.agents` order (not the
/// recent order, and not the `priority` one either: that would put
/// `block-new` first and rank the idle rows by `state_change_seq`).
/// `idle-unknown` has no idle age; `idle-stale` is past the default
/// 120-minute stale edge.
fn recent_sort_snapshot() -> ClientShellSnapshot {
    two_space_agent_snapshot(&[
        (
            "pane_1",
            "ws_1",
            "idle-old",
            AgentStatus::Idle,
            9,
            Some(600),
        ),
        ("pane_2", "ws_1", "work-old", AgentStatus::Working, 2, None),
        ("pane_3", "ws_2", "idle-new", AgentStatus::Idle, 1, Some(30)),
        ("pane_4", "ws_2", "block-new", AgentStatus::Blocked, 7, None),
        ("pane_5", "ws_1", "work-new", AgentStatus::Working, 8, None),
        (
            "pane_6",
            "ws_2",
            "idle-unknown",
            AgentStatus::Idle,
            10,
            None,
        ),
        (
            "pane_7",
            "ws_1",
            "idle-stale",
            AgentStatus::Idle,
            11,
            Some(9_000),
        ),
    ])
}

const RECENT_SORT_NAMES: [&str; 7] = [
    "idle-old",
    "work-old",
    "idle-new",
    "block-new",
    "work-new",
    "idle-unknown",
    "idle-stale",
];

fn recent_sort_state(dim_stale_agents: bool) -> ClientShellState {
    let mut config = Config::default();
    config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Recent;
    config.ui.dim_stale_agents = dim_stale_agents;
    stale_row_state(&config, recent_sort_snapshot())
}

/// The agent panel header's sort label, read off the drawn frame.
fn recent_sort_header_label(state: &mut ClientShellState) -> String {
    let frame = state.compose(106, 60).expect("agent panel frame");
    let toggle = state.hits.agent_sort_toggle;
    assert!(!toggle.is_empty(), "the sort label is drawn and clickable");
    frame_rows(&frame)[toggle.y as usize]
        .chars()
        .skip(toggle.x as usize)
        .take(toggle.width as usize)
        .collect()
}

fn recent_sort_preferences_path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "herdr-shell-recent-sort-{tag}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

/// What a fresh client on the same endpoint loads from `path`: the sort
/// and whether it counts as a saved (manual) choice.
fn recent_sort_reloaded(path: &std::path::Path) -> (crate::config::AgentPanelSortConfig, bool) {
    let reloaded = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.into()),
    );
    (
        reloaded.config.agent_panel_sort,
        reloaded.agent_panel_sort_manual,
    )
}

/// From each drawn row in turn, the agent `action` (`NextAgent` or
/// `PreviousAgent`) focuses next, as names.
fn recent_sort_relative_walk(
    state: &mut ClientShellState,
    drawn: &[&str],
    action: crate::input::KeybindAction,
) -> Vec<String> {
    let agents = state.snapshot.as_deref().expect("snapshot").agents.clone();
    let pane_of = |name: &str| {
        agents
            .iter()
            .find(|agent| agent.name.as_deref() == Some(name))
            .map(|agent| agent.pane_id.clone())
            .expect("fixture agent")
    };
    drawn
        .iter()
        .map(|name| {
            state
                .snapshot
                .as_deref_mut()
                .expect("snapshot")
                .focused_pane_id = Some(pane_of(name));
            let Some(crate::api::schema::Method::PaneFocus(target)) =
                state.endpoint_method_for_action(action)
            else {
                panic!("{action:?} from {name} must focus a pane");
            };
            agents
                .iter()
                .find(|agent| agent.pane_id == target.pane_id)
                .and_then(|agent| agent.name.clone())
                .expect("focused agent name")
        })
        .collect()
}

#[test]
fn recent_sort_draws_live_agents_by_newest_change_then_idle_by_shortest_idle() {
    let mut state = recent_sort_state(true);
    assert_eq!(recent_sort_header_label(&mut state), "recent");
    let drawn = stale_row_drawn_order(&mut state, &RECENT_SORT_NAMES);
    assert_eq!(
        drawn,
        [
            "work-new",
            "block-new",
            "work-old",
            "idle-new",
            "idle-old",
            "idle-unknown",
            "idle-stale",
        ]
    );
    assert_eq!(
        stale_row_keyboard_order(&mut state, &RECENT_SORT_NAMES),
        drawn
    );
    let mut next = drawn[1..].to_vec();
    next.push(drawn[0]);
    assert_eq!(
        recent_sort_relative_walk(&mut state, &drawn, crate::input::KeybindAction::NextAgent),
        next
    );
    let mut previous = vec![drawn[drawn.len() - 1]];
    previous.extend_from_slice(&drawn[..drawn.len() - 1]);
    assert_eq!(
        recent_sort_relative_walk(
            &mut state,
            &drawn,
            crate::input::KeybindAction::PreviousAgent
        ),
        previous
    );
}

/// The #353 sink is consistent in `recent`: a stale row drops below every
/// live row, including the unknown-age one. With `dim_stale_agents` off
/// it keeps its `idle_seconds` place, still after every shorter idle.
#[test]
fn recent_sort_sinks_stale_rows_only_while_dim_stale_agents_is_on() {
    let mut state = recent_sort_state(false);
    let drawn = stale_row_drawn_order(&mut state, &RECENT_SORT_NAMES);
    assert_eq!(
        drawn,
        [
            "work-new",
            "block-new",
            "work-old",
            "idle-new",
            "idle-old",
            "idle-stale",
            "idle-unknown",
        ]
    );
    assert_eq!(
        stale_row_keyboard_order(&mut state, &RECENT_SORT_NAMES),
        drawn
    );
}

#[test]
fn cycle_agent_panel_sort_key_recent_sort_repaints_and_persists_each_step() {
    use crate::config::AgentPanelSortConfig;

    let path = recent_sort_preferences_path("key");
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone()),
    );
    state.set_snapshot(Box::new(recent_sort_snapshot()));
    state.set_pane_surface(surface());
    assert_eq!(recent_sort_header_label(&mut state), "grouped");

    for (sort, label) in [
        (AgentPanelSortConfig::Priority, "priority"),
        (AgentPanelSortConfig::Recent, "recent"),
        (AgentPanelSortConfig::Spaces, "grouped"),
    ] {
        let mut outcome = ClientShellInput::default();
        state.record_binding(
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::CycleAgentPanelSort),
            &mut outcome,
        );
        assert!(outcome.repaint, "cycling to {label} must repaint");
        assert!(outcome.actions.is_empty(), "the sort is client-local");
        assert_eq!(recent_sort_header_label(&mut state), label);
        assert_eq!(recent_sort_reloaded(&path), (sort, true));
    }

    // The default binding reaches the same action: prefix, then `a`.
    state.mode = ClientShellMode::Navigate;
    state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('a'),
        KeyModifiers::empty(),
    ))]);
    assert_eq!(recent_sort_header_label(&mut state), "priority");
    assert_eq!(
        recent_sort_reloaded(&path),
        (AgentPanelSortConfig::Priority, true)
    );
    std::fs::remove_file(path).expect("remove agent sort preferences");
}

#[test]
fn agent_sort_header_click_recent_sort_walks_all_three_values() {
    use crate::config::AgentPanelSortConfig;

    let path = recent_sort_preferences_path("click");
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone()),
    );
    state.set_snapshot(Box::new(recent_sort_snapshot()));
    state.set_pane_surface(surface());

    for (sort, label) in [
        (AgentPanelSortConfig::Priority, "priority"),
        (AgentPanelSortConfig::Recent, "recent"),
        (AgentPanelSortConfig::Spaces, "grouped"),
    ] {
        state.compose(106, 60).expect("agent panel frame");
        let toggle = state.hits.agent_sort_toggle;
        let click =
            state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: toggle.x,
                row: toggle.y,
                modifiers: KeyModifiers::empty(),
            })]);
        assert!(click.repaint, "clicking to {label} must repaint");
        assert!(click.actions.is_empty(), "the sort is client-local");
        assert_eq!(recent_sort_header_label(&mut state), label);
        assert_eq!(recent_sort_reloaded(&path), (sort, true));
        if sort == AgentPanelSortConfig::Recent {
            assert_eq!(
                stale_row_drawn_order(&mut state, &RECENT_SORT_NAMES)[..3],
                ["work-new", "block-new", "work-old"]
            );
        }
    }
    std::fs::remove_file(path).expect("remove agent sort preferences");
}

/// A newer client may save a sort this build does not know. Loading it
/// must keep every other saved choice, fall back to the config default
/// for the sort (not the older-client `agent_panel_sort` key: the v2 key
/// wins), and say so exactly once. Saving another preference must write
/// the newer client's keys back verbatim; only an explicit sort choice here
/// replaces them.
#[test]
fn recent_sort_unknown_saved_value_loads_config_default_with_one_warning() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tracing_subscriber::layer::SubscriberExt as _;

    struct WarnCounter(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnCounter {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if *event.metadata().level() == tracing::Level::WARN {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    let path = recent_sort_preferences_path("unknown");
    std::fs::write(
        &path,
        r#"{"sidebar_width":30,"view_mode":"flat","agent_panel_sort":"spaces","agent_panel_sort_v2":"oldest"}"#,
    )
    .expect("write preferences");
    let mut config = Config::default();
    config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
    let config = ClientShellConfig::from_config(&config);
    let warnings = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(WarnCounter(warnings.clone()));
    let mut state = tracing::subscriber::with_default(subscriber, || {
        ClientShellState::new(config.with_preferences_path(path.clone()))
    });

    assert_eq!(
        state.config.agent_panel_sort,
        crate::config::AgentPanelSortConfig::Priority
    );
    assert!(!state.agent_panel_sort_manual);
    assert_eq!(
        (state.sidebar_width, state.sidebar_width_manual),
        (30, true)
    );
    assert_eq!(state.view_mode, crate::config::ViewMode::Flat);
    assert_eq!(warnings.load(Ordering::SeqCst), 1);

    let saved = |path: &std::path::Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read preferences"))
            .expect("preferences json")
    };
    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CycleViewMode),
        &mut outcome,
    );
    assert_eq!(
        saved(&path),
        serde_json::json!({
            "sidebar_width": 30,
            "view_mode": "folders",
            "agent_panel_sort": "spaces",
            "agent_panel_sort_v2": "oldest",
        })
    );

    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CycleAgentPanelSort),
        &mut outcome,
    );
    assert_eq!(
        saved(&path),
        serde_json::json!({
            "sidebar_width": 30,
            "view_mode": "folders",
            "agent_panel_sort_v2": "recent",
        })
    );
    std::fs::remove_file(path).expect("remove agent sort preferences");
}

#[test]
fn workspace_actions_preserve_selected_target_and_client_confirmation() {
    let mut snapshot = snapshot();
    let mut second = snapshot.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.number = 2;
    second.label = "second".into();
    second.focused = false;
    snapshot.workspaces.push(second);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot));
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_2");

    let rename = state.handle_raw_events(vec![RawInputEvent::Key(crate::input::TerminalKey::new(
        KeyCode::Char('w'),
        KeyModifiers::SHIFT,
    ))]);
    assert!(rename.actions.is_empty());
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::Workspace { workspace_id },
            ..
        })) if workspace_id == "ws_2"
    ));
    assert!(state.handle_input_bytes(&[0x15]).actions.is_empty());
    assert!(state.handle_input_bytes(b"renamed").actions.is_empty());
    let save = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &save.actions[..] else {
        panic!("workspace rename should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceRename(params)
            if params.workspace_id == "ws_2" && params.label == "renamed"
    ));

    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_2");
    let mut close = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CloseWorkspace),
        &mut close,
    );
    assert!(close.actions.is_empty());
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::ConfirmClose(ClientConfirmCloseOverlay {
            workspace_id,
            ..
        })) if workspace_id == "ws_2"
    ));
    let confirm = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &confirm.actions[..] else {
        panic!("workspace confirmation should use endpoint API");
    };
    // Default view mode is `ViewMode::Repo`, the one view that renders a
    // worktree group nested, so the close is allowed to drag the group.
    assert_eq!(state.view_mode, crate::config::ViewMode::Repo);
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceClose(params)
            if params.workspace_id == "ws_2" && !params.close_group
    ));
}

#[test]
fn desktop_workspace_navigation_reveals_overflowing_selection() {
    let mut projected = snapshot();
    for index in 2..=8 {
        let mut workspace = projected.workspaces[0].clone();
        workspace.workspace_id = format!("ws_{index}");
        workspace.number = index;
        workspace.label = format!("workspace-{index}");
        workspace.focused = false;
        projected.workspaces.push(workspace);
    }
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_1");
    state.compose(106, 12).expect("overflowing sidebar");

    for _ in 0..6 {
        state.handle_input_bytes(b"\x1b[B");
        state.compose(106, 12).expect("revealed workspace");
        let selected = state
            .navigate_workspace_id
            .as_ref()
            .expect("selection")
            .workspace_id
            .as_str();
        assert!(
            state
                .hits
                .workspaces
                .iter()
                .any(|hit| hit.workspace_id == selected),
            "{selected} should remain visible"
        );
    }
}

#[test]
fn named_workspace_overlay_targets_projected_source_workspace() {
    let mut config = Config::default();
    config.ui.prompt_new_workspace_name = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
        &mut open,
    );
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            input: value,
            target: ClientRenameTarget::NewWorkspace {
                source_workspace_id,
                ..
            },
            ..
        })) if value.as_str() == "repo" && source_workspace_id.as_deref() == Some("ws_1")
    ));
    let create = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &create.actions[..] else {
        panic!("named workspace should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceCreate(params)
            if params.source_workspace_id.as_deref() == Some("ws_1")
                && params.cwd.as_deref() == Some("/repo")
                && params.label.is_none()
    ));
}

/// In Navigate mode the `cmd+n`/`keys.workspace_new` group must come from
/// the SAME workspace used as `source_workspace_id` (`workspace_action_id()`,
/// i.e. the selected `navigate_workspace_id`) -- never from whichever
/// workspace the server reports as `focused`, which can be a different
/// one while browsing (cubic review, ceo-bora#303 PR #32).
#[test]
fn named_workspace_overlay_group_matches_the_selected_not_the_focused_workspace() {
    let mut config = Config::default();
    config.ui.prompt_new_workspace_name = true;
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut projected = snapshot();
    // The server-focused workspace ("ws_1") sits in "alpha"; the SELECTED
    // (navigate_workspace_id) workspace ("ws_2") sits in "beta".
    projected.workspaces[0].visual_group = Some("alpha".into());
    let mut second = projected.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.number = 2;
    second.label = "second".into();
    second.focused = false;
    second.visual_group = Some("beta".into());
    projected.workspaces.push(second);

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_2");

    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
        &mut open,
    );
    assert!(matches!(
        state.overlay.as_ref(),
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::NewWorkspace {
                source_workspace_id,
                group: Some(group),
                ..
            },
            ..
        })) if source_workspace_id.as_deref() == Some("ws_2") && group == "beta"
    ));

    // Same rule on the direct-create path (`prompt_new_workspace_name = false`).
    let mut direct_config = Config::default();
    direct_config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut direct_state = ClientShellState::new(ClientShellConfig::from_config(&direct_config));
    direct_state.set_snapshot(Box::new(state.snapshot.as_deref().unwrap().clone()));
    direct_state.mode = ClientShellMode::Navigate;
    direct_state.navigate_workspace_id =
        direct_state.navigation_target(&ClientEndpointId::Local, "ws_2");
    let mut create = ClientShellInput::default();
    direct_state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorkspace),
        &mut create,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &create.actions[..] else {
        panic!("direct-create new workspace should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceCreate(params)
            if params.source_workspace_id.as_deref() == Some("ws_2")
                && params.group.as_deref() == Some("beta")
    ));
}

#[test]
fn navigate_mode_selects_workspace_locally_then_focuses_by_stable_id() {
    let mut snapshot = snapshot();
    let mut second = snapshot.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.number = 2;
    second.label = "second".into();
    second.focused = false;
    snapshot.workspaces.push(second);
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot));
    state.set_pane_surface(surface());

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    let enter_navigate = state.handle_input_bytes(b"w");
    assert!(enter_navigate.repaint);
    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_1")
    );

    let invalid = state.handle_input_bytes(b"9");
    assert!(invalid.actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Navigate);
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_1")
    );

    let move_selection = state.handle_input_bytes(b"\x1b[B");
    assert!(move_selection.actions.is_empty());
    assert_eq!(
        state.navigate_workspace_id,
        state.navigation_target(&ClientEndpointId::Local, "ws_2")
    );
    let frame = state.compose(106, 20).expect("navigate frame");
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
    assert!(text.contains("second"));
    assert!(text.contains("NAVIGATE"));

    let focus = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &focus.actions[..] else {
        panic!("selected workspace should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceFocus(target)
            if target.workspace_id == "ws_2"
    ));
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn worktree_create_previews_the_endpoint_owned_checkout_path() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut prepare = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorktree),
        &mut prepare,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &prepare.actions[..] else {
        panic!("new worktree should prepare through worktree.list");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorktreeList(params)
            if params.workspace_id.as_deref() == Some("ws_1")
    ));
    let request_id = request.id.clone();
    assert!(
        state
            .handle_endpoint_result("boot-1", &request_id, Ok(worktree_list_result(None)))
            .0
    );
    let frame = state.compose(106, 30).expect("new worktree modal");
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
    assert!(text.contains("new worktree"));
    assert!(text.contains("create and open"));
    assert!(frame.cursor.as_ref().is_some_and(|cursor| cursor.visible));

    assert!(state
        .handle_input_bytes(b"feature/client-shell")
        .actions
        .is_empty());
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::WorktreeCreate(create))
            if create.checkout_path
                == "/tmp/herdr-worktrees/repo/feature-client-shell"
    ));
    let submit = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &submit.actions[..] else {
        panic!("worktree create should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorktreeCreate(params)
            if params.workspace_id.as_deref() == Some("ws_1")
                && params.branch.as_deref() == Some("feature/client-shell")
                && params.path.is_none()
                && !params.focus
    ));
}

#[test]
fn unavailable_worktree_create_does_not_wedge_the_overlay() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut prepare = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorktree),
        &mut prepare,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &prepare.actions[..] else {
        panic!("new worktree should prepare through worktree.list");
    };
    state.handle_endpoint_result("boot-1", &request.id, Ok(worktree_list_result(None)));
    state.set_endpoint_methods(Some(vec!["worktree.list".into()]));
    state.handle_input_bytes(b"feature/unavailable");

    let submit = state.handle_input_bytes(b"\r");

    assert!(submit.actions.is_empty());
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::WorktreeCreate(create)) if !create.creating
    ));
    assert!(state
        .visible_endpoint_notice
        .as_ref()
        .is_some_and(|notice| notice.key.code == "worktree.create"));
}

#[test]
fn worktree_action_errors_expire_without_more_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    let mut guard = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::RemoveWorktree),
        &mut guard,
    );
    let message = "This workspace is not a Bora-managed worktree checkout.";
    assert_eq!(state.endpoint_error.as_deref(), Some(message));

    let deadline = state.endpoint_error_deadline.expect("deadline");
    assert!(!state.tick_endpoint_error(deadline - std::time::Duration::from_secs(1)));
    assert_eq!(state.endpoint_error.as_deref(), Some(message));

    assert!(state.tick_endpoint_error(deadline + std::time::Duration::from_millis(1)));
    assert!(state.endpoint_error.is_none());

    // A repeated identical message must start a fresh lifetime instead of
    // inheriting the earlier deadline.
    let before_repeat = std::time::Instant::now();
    state.set_endpoint_error(message);
    assert!(
        state.endpoint_error_deadline.expect("deadline")
            >= before_repeat + std::time::Duration::from_secs(5)
    );
}

#[test]
fn worktree_prepare_rejection_notice_expires() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut prepare = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NewWorktree),
        &mut prepare,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &prepare.actions[..] else {
        panic!("new worktree should prepare through worktree.list");
    };
    let request_id = request.id.clone();
    state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("not_git_worktree".into()),
            message: "Bora worktree actions require a workspace inside a Git work tree".into(),
        }),
    );
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("rejection notice");
    assert!(notice.key.code.contains("not_git_worktree"));
    let deadline = notice.deadline;

    let (_, repaint) = state.tick_notifications(deadline + std::time::Duration::from_millis(1));
    assert!(repaint);
    assert!(state.visible_endpoint_notice.is_none());
}

#[test]
fn worktree_open_filters_and_clicks_a_stable_public_entry() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut prepare = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::OpenWorktree),
        &mut prepare,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &prepare.actions[..] else {
        panic!("open worktree should prepare through worktree.list");
    };
    let request_id = request.id.clone();
    state.handle_endpoint_result("boot-1", &request_id, Ok(worktree_list_result(None)));
    let frame = state.compose(106, 30).expect("open worktree modal");
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
    assert!(text.contains("feature"));
    let row = state.hits.worktree_rows[0].0;
    let open = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: row.x + 2,
        row: row.y,
        modifiers: KeyModifiers::empty(),
    })]);
    let [ClientShellAction::Endpoint { request, .. }] = &open.actions[..] else {
        panic!("worktree row should open through endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorktreeOpen(params)
            if params.workspace_id.as_deref() == Some("ws_1")
                && params.path.as_deref() == Some("/repo-feature")
                && params.focus
    ));
}

#[test]
fn worktree_remove_escalates_recoverable_failure_to_force_confirmation() {
    for (code, message, expect_force) in [
        ("dirty_worktree_requires_force", "dirty worktree", true),
        (
            "worktree_remove_failed",
            "fatal: '/repo-feature' is not a working tree",
            true,
        ),
        ("worktree_remove_failed", "Permission denied", false),
        ("server_unavailable", "is not a working tree", false),
    ] {
        let mut snapshot = snapshot();
        snapshot.workspaces[0].worktree = Some(ClientShellWorktree {
            key: "repo-key".into(),
            label: "repo".into(),
            is_linked_worktree: true,
        });
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
        state.set_snapshot(Box::new(snapshot));
        state.set_pane_surface(surface());
        let mut prepare = ClientShellInput::default();
        state.record_binding(
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::RemoveWorktree),
            &mut prepare,
        );
        let [ClientShellAction::Endpoint { request, .. }] = &prepare.actions[..] else {
            panic!("remove worktree should prepare through worktree.list");
        };
        let request_id = request.id.clone();
        state.handle_endpoint_result(
            "boot-1",
            &request_id,
            Ok(worktree_list_result(Some("ws_1"))),
        );
        let remove = state.handle_input_bytes(b"\r");
        let [ClientShellAction::Endpoint { request, .. }] = &remove.actions[..] else {
            panic!("worktree remove should use endpoint API");
        };
        assert!(matches!(
            &request.method,
            crate::api::schema::Method::WorktreeRemove(params)
                if params.workspace_id == "ws_1" && !params.force
        ));
        let request_id = request.id.clone();
        let (_, actions) = state.handle_endpoint_result(
            "boot-1",
            &request_id,
            Err(ClientShellEndpointError {
                code: Some(code.into()),
                message: message.into(),
            }),
        );
        assert!(actions.is_empty(), "failure must not retry automatically");
        let Some(ClientShellOverlay::WorktreeRemove(remove)) = &state.overlay else {
            panic!("failed remove should keep its confirmation");
        };
        assert!(!remove.removing);
        assert_eq!(remove.force_confirmation, expect_force);
        if !expect_force {
            assert_eq!(remove.error.as_deref(), Some(message));
            continue;
        }
        assert!(remove.error.is_none());
        let frame = state.compose(106, 30).expect("force remove modal");
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
        assert!(text.contains("delete anyway"));
        assert!(text.contains("permanently deleted"));
        let force = state.handle_input_bytes(b"\r");
        let [ClientShellAction::Endpoint { request, .. }] = &force.actions[..] else {
            panic!("forced worktree remove should use endpoint API");
        };
        assert!(matches!(
            &request.method,
            crate::api::schema::Method::WorktreeRemove(params)
                if params.workspace_id == "ws_1" && params.force
        ));
        let request_id = request.id.clone();
        let (_, actions) = state.handle_endpoint_result(
            "boot-1",
            &request_id,
            Err(ClientShellEndpointError {
                code: Some("worktree_remove_failed".into()),
                message: "fatal: '/repo-feature' is not a working tree".into(),
            }),
        );
        assert!(actions.is_empty());
        let Some(ClientShellOverlay::WorktreeRemove(remove)) = &state.overlay else {
            panic!("forced failure should keep its confirmation");
        };
        assert!(!remove.removing);
        assert_eq!(
            remove.error.as_deref(),
            Some("fatal: '/repo-feature' is not a working tree")
        );
        state.handle_input_bytes(b"\x1b");
        assert!(state.overlay.is_none());
    }
}

#[test]
fn semantic_notifications_use_client_policy_and_stable_navigation_targets() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.toast_delivery = crate::config::ToastDelivery::Herdr;
    config.toast_delay_seconds = 0;
    let mut state = ClientShellState::new(config);
    let mut projected = snapshot();
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_2".into(),
        workspace_id: "ws_2".into(),
        tab_id: "tab_2".into(),
        name: None,
        display_agent: Some("codex".into()),
        agent: Some("codex".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Blocked,
        state_change_seq: 1,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let now = std::time::Instant::now();
    let (effects, repaint) = state.receive_notification(
        &ClientEndpointId::Local,
        SemanticNotification {
            kind: SemanticNotificationKind::NeedsAttention,
            title: "codex needs attention".into(),
            body: Some("other · 2".into()),
            sound: Some(SemanticNotificationSound::Request),
            agent: Some("codex".into()),
            workspace_id: Some("ws_2".into()),
            tab_id: Some("tab_2".into()),
            pane_id: Some("pane_2".into()),
            position: None,
        },
        now,
    );
    assert!(repaint);
    assert!(matches!(
        effects.as_slice(),
        [ClientShellNotificationEffect::Sound {
            sound: crate::sound::Sound::Request,
            ..
        }]
    ));
    let frame = state.compose(100, 28).expect("notification frame");
    let rendered = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("codex needs attention"));
    let hit = state.hits.notification_toast;
    let click = || {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.mode = ClientShellMode::Navigate;
    let ignored = state.handle_raw_events(vec![click()]);
    assert!(ignored.actions.is_empty());
    assert!(state.visible_notification.is_some());

    state.mode = ClientShellMode::Terminal;
    let outcome = state.handle_raw_events(vec![click()]);
    assert!(outcome.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneFocus(params)
                    if params.pane_id == "pane_2"
            )
    )));
    assert!(state.visible_notification.is_none());

    state.receive_notification(
        &ClientEndpointId::Local,
        SemanticNotification {
            kind: SemanticNotificationKind::NeedsAttention,
            title: "codex needs attention".into(),
            body: None,
            sound: None,
            agent: Some("codex".into()),
            workspace_id: Some("ws_2".into()),
            tab_id: Some("tab_2".into()),
            pane_id: Some("pane_2".into()),
            position: None,
        },
        now,
    );
    let mut keybind = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::OpenNotificationTarget),
        &mut keybind,
    );
    assert!(keybind.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneFocus(params)
                    if params.pane_id == "pane_2"
            )
    )));
    assert!(state.visible_notification.is_none());

    state.receive_notification(
        &ClientEndpointId::Local,
        SemanticNotification {
            kind: SemanticNotificationKind::NeedsAttention,
            title: "first".into(),
            body: None,
            sound: None,
            agent: Some("codex".into()),
            workspace_id: Some("ws_2".into()),
            tab_id: Some("tab_2".into()),
            pane_id: Some("pane_2".into()),
            position: None,
        },
        now,
    );
    assert!(state.visible_notification.is_some());
    state.config.toast_delay_seconds = 1;
    let (_, repaint) = state.receive_notification(
        &ClientEndpointId::Local,
        SemanticNotification {
            kind: SemanticNotificationKind::NeedsAttention,
            title: "replacement".into(),
            body: None,
            sound: None,
            agent: Some("codex".into()),
            workspace_id: Some("ws_2".into()),
            tab_id: Some("tab_2".into()),
            pane_id: Some("pane_2".into()),
            position: None,
        },
        now,
    );
    assert!(repaint);
    assert!(state.visible_notification.is_none());
    assert_eq!(state.pending_notifications.len(), 1);
}

/// Root checkout `ws_1` plus a linked worktree `ws_2` sharing its
/// `worktree.key`, with the child parked in an unrelated sidebar
/// folder -- the shape that made a group close reach rows the user
/// could not see.
fn worktree_group_state(view_mode: crate::config::ViewMode) -> ClientShellState {
    let mut config = Config::default();
    config.ui.view_mode = view_mode;
    let mut projected = snapshot();
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    projected.workspaces.push(ClientShellWorkspace {
        workspace_id: "ws_2".into(),
        active_tab_id: "tab_ws2".into(),
        new_workspace_cwd: "/repo/feature".into(),
        number: 2,
        label: "repo-feature".into(),
        custom_label: false,
        branch: Some("worktree/feature".into()),
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: Some(ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree: true,
        }),
        visual_group: Some("foxtrot".into()),
        open_human_asks: 0,
        focused: false,
        agent_status: AgentStatus::Idle,
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&ClientEndpointId::Local, "ws_1");
    state
}

fn close_workspace_confirm_title(state: &mut ClientShellState) -> String {
    let mut close = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CloseWorkspace),
        &mut close,
    );
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ConfirmClose(confirm)) => confirm.title.clone(),
        _ => panic!("expected a close confirmation overlay"),
    }
}

/// Only `ViewMode::Repo` renders the worktree group nested, so only
/// there may a close of the root checkout drag the linked worktrees
/// with it. In Folders the child is a separate top-level row in
/// another folder: closing the parent must close the parent alone.
#[test]
fn folders_view_close_confirms_and_closes_only_the_clicked_workspace() {
    let mut state = worktree_group_state(crate::config::ViewMode::Folders);
    assert_eq!(
        close_workspace_confirm_title(&mut state),
        "Close workspace?"
    );

    let confirm = state.handle_input_bytes(b"\r");
    let [ClientShellAction::Endpoint { request, .. }] = &confirm.actions[..] else {
        panic!("confirmation should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::WorkspaceClose(params)
            if params.workspace_id == "ws_1" && !params.close_group
    ));
}

#[test]
fn repo_view_close_still_confirms_and_closes_the_whole_worktree_group() {
    let mut state = worktree_group_state(crate::config::ViewMode::Repo);
    assert_eq!(
        close_workspace_confirm_title(&mut state),
        "Close worktree group?"
    );

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

#[test]
fn folders_view_workspace_context_menu_offers_close_not_close_group() {
    let labels = |state: &ClientShellState| match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => menu
            .items()
            .into_iter()
            .map(|item| item.label.into_owned())
            .collect::<Vec<_>>(),
        _ => panic!("expected a context menu"),
    };

    let mut folders = worktree_group_state(crate::config::ViewMode::Folders);
    folders.open_workspace_context_menu("ws_1".into(), 0, 0);
    let folders_labels = labels(&folders);
    assert!(folders_labels.iter().any(|label| label == "Close"));
    assert!(!folders_labels.iter().any(|label| label == "Close group"));
    assert!(folders_labels.iter().any(|label| label == "New worktree"));

    let mut repo = worktree_group_state(crate::config::ViewMode::Repo);
    repo.open_workspace_context_menu("ws_1".into(), 0, 0);
    let repo_labels = labels(&repo);
    assert!(repo_labels.iter().any(|label| label == "Close group"));
    assert!(!repo_labels.iter().any(|label| label == "Close"));
}

/// `prefix+]`/`[` must walk the list the user is looking at. In Folders
/// view that is `folders_entries` -- loose rows first, then each folder's
/// members -- and a collapsed folder contributes nothing to it. Reading
/// the Repo-nested order here made next/previous jump to rows a collapsed
/// folder was hiding and skip the ones it showed.
#[test]
fn folders_view_workspace_navigation_follows_the_rendered_order() {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let projected = folders_nav_snapshot();

    let next_target = |state: &mut ClientShellState| {
        let mut input = ClientShellInput::default();
        state.record_binding(
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextWorkspace),
            &mut input,
        );
        match &input.actions[..] {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                crate::api::schema::Method::WorkspaceFocus(target) => target.workspace_id.clone(),
                other => panic!("expected workspace.focus, got {other:?}"),
            },
            other => panic!("expected one endpoint action, got {other:?}"),
        }
    };

    let mut folders = ClientShellState::new(ClientShellConfig::from_config(&config));
    folders.set_snapshot(Box::new(projected.clone()));
    assert_eq!(next_target(&mut folders), "ws_3");

    // Collapsing "a" removes its member from the walk entirely, so the
    // step after ws_3 wraps to ws_1 instead of landing on the hidden ws_2.
    folders.collapsed_groups.insert("vg:a".into());
    let mut focused_ws_3 = projected.clone();
    focused_ws_3.revision = 2;
    focused_ws_3.focused_workspace_id = Some("ws_3".into());
    focused_ws_3.workspaces[0].focused = false;
    focused_ws_3.workspaces[2].focused = true;
    folders.set_snapshot(Box::new(focused_ws_3));
    assert_eq!(next_target(&mut folders), "ws_1");

    // Repo view still walks the nested/workspace-vec order.
    let mut repo_config = Config::default();
    repo_config.ui.view_mode = crate::config::ViewMode::Repo;
    let mut repo = ClientShellState::new(ClientShellConfig::from_config(&repo_config));
    repo.set_snapshot(Box::new(projected));
    assert_eq!(next_target(&mut repo), "ws_2");
}

/// ws_1 (loose, focused), ws_2 (folder "a"), ws_3 (loose). Workspace-vec
/// order is 1,2,3; the rendered Folders order is 1, 3, header "a", 2.
pub(in crate::client::shell) fn folders_nav_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    for (index, group) in [(2, Some("a")), (3, None)] {
        let mut workspace = projected.workspaces[0].clone();
        workspace.workspace_id = format!("ws_{index}");
        workspace.number = index;
        workspace.label = format!("workspace-{index}");
        workspace.focused = false;
        workspace.visual_group = group.map(str::to_owned);
        projected.workspaces.push(workspace);
    }
    projected
}

/// `workspace_scroll` is an index into the RENDERED rows, and Folders
/// renders group headers the navigation list drops. Revealing ws_2 -- the
/// last of the four rendered rows -- must scroll to row 3, not to its
/// workspace-only position 2, or the renderer leaves it off-screen.
#[test]
fn folders_view_reveal_scrolls_past_the_group_header_rows() {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(folders_nav_snapshot()));
    state.hits.workspace_max_scroll = 10;

    state.reveal_workspace("ws_2");

    assert_eq!(state.workspace_scroll, 3);
}

/// Collapsed draws neither headers nor grouping, so the header-aware offset
/// above must not apply there: ws_2 is the second collapsed row, not the
/// fourth (cubic review, PR #40).
#[test]
fn collapsed_sidebar_reveal_uses_the_flat_row_index() {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(folders_nav_snapshot()));
    state.hits.workspace_max_scroll = 10;
    state.sidebar_collapsed = true;

    state.reveal_workspace("ws_2");

    assert_eq!(state.workspace_scroll, 1);
}

/// A collapsed folder hides the focused workspace, so it is absent from
/// the walk. Stepping forward from "nowhere" must land on the FIRST
/// visible row; starting from index 0 made it land on the second and skip
/// the first outright.
#[test]
fn folders_view_navigation_from_a_hidden_focused_workspace_lands_on_the_first_row() {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let mut projected = folders_nav_snapshot();
    // Focus ws_2, the member of folder "a", then collapse "a".
    projected.focused_workspace_id = Some("ws_2".into());
    projected.workspaces[0].focused = false;
    projected.workspaces[1].focused = true;
    state.set_snapshot(Box::new(projected));
    state.collapsed_groups.insert("vg:a".into());

    let mut next = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::NextWorkspace),
        &mut next,
    );
    assert!(matches!(
        &next.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_1"
            )
    ));

    let mut previous = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::PreviousWorkspace),
        &mut previous,
    );
    assert!(matches!(
        &previous.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceFocus(target) if target.workspace_id == "ws_3"
            )
    ));
}

/// Both collapsed renderers -- `render_collapsed_sidebar` (one machine) and
/// `endpoint_sidebar::render_collapsed` (several) -- walk `snapshot.workspaces`
/// directly and draw neither folder headers nor worktree nesting. So a
/// collapsed sidebar navigates in snapshot-vec order no matter which view mode
/// is live, and a collapsed folder hides nothing (cubic review, PR #40).
#[test]
fn collapsed_sidebar_navigates_in_snapshot_order_whatever_the_view_mode() {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(folders_nav_snapshot()));
    // Folder "a" holds ws_2 and is collapsed: expanded Folders would render
    // ws_1, ws_3, header "a" -- and never ws_2.
    state.collapsed_groups.insert("vg:a".into());

    let second_row = |state: &mut ClientShellState| {
        let mut input = ClientShellInput::default();
        state.record_binding(
            crate::input::KeybindMatch::Action(crate::input::KeybindAction::SwitchWorkspace(1)),
            &mut input,
        );
        match &input.actions[..] {
            [ClientShellAction::Endpoint { request, .. }] => match &request.method {
                crate::api::schema::Method::WorkspaceFocus(target) => target.workspace_id.clone(),
                other => panic!("expected workspace.focus, got {other:?}"),
            },
            other => panic!("expected one endpoint action, got {other:?}"),
        }
    };

    // Expanded: Folders order, so row 2 is the other loose workspace.
    assert_eq!(second_row(&mut state), "ws_3");

    // Collapsed: snapshot-vec order, so row 2 is ws_2 -- the very workspace the
    // collapsed folder hid from the expanded list.
    state.sidebar_collapsed = true;
    assert_eq!(second_row(&mut state), "ws_2");
}

/// ceo-bora#357 fixture: one unfocused agent per `(name, kind, status,
/// idle_seconds)`, each on its own pane in `ws_1`/`tab_1`.
fn agent_mark_snapshot(
    revision: u64,
    rows: &[(&str, &str, AgentStatus, Option<u64>)],
) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.revision = revision;
    projected.panes.clear();
    projected.agents.clear();
    for (index, (name, kind, status, idle_seconds)) in rows.iter().enumerate() {
        let pane_id = format!("pane_{}", index + 1);
        let mut pane = snapshot().panes[0].clone();
        pane.pane_id = pane_id.clone();
        pane.focused = false;
        pane.idle_seconds = *idle_seconds;
        projected.panes.push(pane);
        projected.agents.push(ClientShellAgent {
            pane_id,
            workspace_id: "ws_1".into(),
            tab_id: "tab_1".into(),
            name: Some((*name).into()),
            display_agent: None,
            agent: Some((*kind).into()),
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            agent_status: *status,
            state_change_seq: revision * 10 + index as u64,
            state_labels: Vec::new(),
            tokens: Vec::new(),
            focused: false,
        });
    }
    projected
}

/// The style of the cell where `needle` starts inside `pane_id`'s agent row.
fn agent_mark_cell(
    state: &ClientShellState,
    frame: &FrameData,
    pane_id: &str,
    needle: &str,
) -> ((u16, u16), ratatui::buffer::Cell) {
    let row = state
        .hits
        .agents
        .iter()
        .find(|(_, hit)| hit == pane_id)
        .unwrap_or_else(|| panic!("{pane_id} agent hit"))
        .0;
    let position = cell_symbol_position(frame, row, needle);
    let cell = frame
        .to_ratatui_buffer()
        .expect("agent sidebar buffer")
        .cell(position)
        .expect("agent row cell")
        .clone();
    (position, cell)
}

/// ceo-bora#357: `agent_mark` draws the kind's glyph in the kind's colour --
/// a configured mark, a built-in one -- and a kind with neither draws
/// nothing: its name sits one space after the state icon, as without the token.
#[test]
fn agent_mark_draws_the_kind_glyph_in_its_color_from_the_rendered_buffer() {
    let config: Config = toml::from_str(
        r##"
[ui.sidebar.agents]
rows = [["state_icon", "agent_mark", "agent"]]

[ui.sidebar.agents.marks]
claude = { glyph = "§", fg = "#89b4fa" }
"##,
    )
    .expect("agent mark config");
    let shell_config = ClientShellConfig::from_config(&config);
    let mauve = shell_config.palette.mauve;
    let mut state = ClientShellState::new(shell_config);
    state.set_snapshot(Box::new(agent_mark_snapshot(
        1,
        &[
            ("claude-agent", "claude", AgentStatus::Idle, None),
            ("pi-agent", "pi", AgentStatus::Idle, None),
            ("devin-agent", "devin", AgentStatus::Idle, None),
        ],
    )));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 30).expect("agent sidebar frame");

    let ((mark_x, mark_y), claude_mark) = agent_mark_cell(&state, &frame, "pane_1", "§");
    assert_eq!(claude_mark.symbol(), "§");
    assert_eq!(
        claude_mark.fg,
        ratatui::style::Color::Rgb(137, 180, 250),
        "{claude_mark:?}"
    );
    let (name, _) = agent_mark_cell(&state, &frame, "pane_1", "claude-agent");
    assert_eq!(
        name,
        (mark_x + 2, mark_y),
        "one space between mark and name"
    );

    let (_, pi_mark) = agent_mark_cell(&state, &frame, "pane_2", "\u{F03FF}");
    assert_eq!(pi_mark.symbol(), "\u{F03FF}");
    assert_eq!(pi_mark.fg, mauve, "{pi_mark:?}");

    let ((name_x, name_y), _) = agent_mark_cell(&state, &frame, "pane_3", "devin-agent");
    let ((icon_x, icon_y), _) = agent_mark_cell(&state, &frame, "pane_3", "○");
    assert_eq!(
        (name_x, name_y),
        (icon_x + 2, icon_y),
        "no mark, no stray space"
    );
}

/// ceo-bora#357: with `title_by_status = true`, a Working, Blocked or Done
/// agent's name takes its `status_color`; an Idle one keeps its #352 tier
/// colour. Off (the default), a Working name keeps the plain `overlay0`.
#[test]
fn title_by_status_colors_working_blocked_done_names_and_idle_keeps_its_tier() {
    let rows = |done: AgentStatus| {
        [
            ("idle-agent", "pi", AgentStatus::Idle, Some(1_800)),
            ("working-agent", "pi", AgentStatus::Working, None),
            ("blocked-agent", "pi", AgentStatus::Blocked, None),
            ("done-agent", "pi", done, Some(1_800)),
        ]
    };
    let render = |title_by_status: bool| {
        let mut config = Config::default();
        config.ui.sidebar.agents.title_by_status = title_by_status;
        let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
        // Working first, then Done: the client shows Done only after it saw
        // the work (a first-seen Done projects to Idle).
        state.set_snapshot(Box::new(agent_mark_snapshot(
            1,
            &rows(AgentStatus::Working),
        )));
        state.set_snapshot(Box::new(agent_mark_snapshot(2, &rows(AgentStatus::Done))));
        state.set_pane_surface(surface());
        let frame = state.compose(106, 30).expect("agent sidebar frame");
        let fg = |pane_id: &str, name: &str| agent_mark_cell(&state, &frame, pane_id, name).1.fg;
        [
            fg("pane_1", "idle-agent"),
            fg("pane_2", "working-agent"),
            fg("pane_3", "blocked-agent"),
            fg("pane_4", "done-agent"),
        ]
    };
    let palette = ClientShellConfig::from_config(&Config::default()).palette;

    assert_eq!(
        render(true),
        [palette.subtext0, palette.yellow, palette.red, palette.teal]
    );
    assert_eq!(
        render(false),
        [
            palette.subtext0,
            palette.overlay0,
            palette.overlay0,
            palette.subtext0
        ]
    );
}
