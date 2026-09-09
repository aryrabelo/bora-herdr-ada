use super::*;

fn folders_config() -> ClientShellConfig {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    ClientShellConfig::from_config(&config)
}

fn folders_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces[0].visual_group = Some("alpha".into());
    // A second pane on ws_1 with a Working agent, so the Folders dots row
    // for ws_1 carries two DIFFERENT glyphs/colors: this is what the
    // "pane_dots" half of the test below actually exercises. The first
    // pane ("pane_1", from `snapshot()`) has no agent entry, so it stays
    // `AgentStatus::Unknown`.
    projected.panes.push(ClientShellPane {
        pane_id: "pane_1b".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        label: None,
        cwd: Some("/repo".into()),
        foreground_cwd: Some("/repo".into()),
        focused: false,
        right_click_passthrough: false,
    });
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_1b".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: None,
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: AgentStatus::Working,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });
    let mut second = projected.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.number = 2;
    second.label = "workspace-2".into();
    second.focused = false;
    second.visual_group = Some("alpha".into());
    let mut third = projected.workspaces[0].clone();
    third.workspace_id = "ws_3".into();
    third.number = 3;
    third.label = "workspace-3".into();
    third.focused = false;
    third.visual_group = None;
    projected.workspaces.push(second);
    projected.workspaces.push(third);
    projected
}

#[test]
fn folders_view_renders_group_header_and_pane_dots_row() {
    let mut state = ClientShellState::new(folders_config());
    state.set_snapshot(Box::new(folders_snapshot()));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 24).expect("folders layout");

    assert_eq!(state.hits.folders_group_headers.len(), 1);
    assert_eq!(state.hits.folders_group_headers[0].1, "vg:alpha");
    // ws_1 and ws_2 (grouped) plus ws_3 (ungrouped) are all still clickable
    // workspace rows -- the Folders render path reuses the same hit type
    // as Flat/Repo so click/drag continue to work for free.
    let ids = state
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.workspace_id.clone())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"ws_1".to_string()));
    assert!(ids.contains(&"ws_2".to_string()));
    assert!(ids.contains(&"ws_3".to_string()));

    // ws_1 has two panes with different agent statuses (pane_1: no agent
    // -> Unknown "·"; pane_1b: Working -> the shared spinner glyph). Both
    // dots must actually render, right-aligned on ws_1's own row, in
    // `snapshot.panes` order -- proving the row really reads per-PANE
    // state off `snapshot.agents`, not a single workspace-level status.
    let ws1_rect = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("ws_1 row")
        .rect;
    let palette = &state.config.palette;
    let unknown_style = (
        crate::client::shell::status_icon(AgentStatus::Unknown, state.config.status_indicators),
        crate::protocol::color_to_u32(crate::client::shell::status_color(
            AgentStatus::Unknown,
            palette,
        )),
    );
    let working_style = (
        crate::client::shell::status_icon(AgentStatus::Working, state.config.status_indicators),
        crate::protocol::color_to_u32(crate::client::shell::status_color(
            AgentStatus::Working,
            palette,
        )),
    );
    // Two dots, one separating space: columns right.saturating_sub(3) and
    // right.saturating_sub(1).
    let first_dot_x = ws1_rect.right().saturating_sub(3);
    let second_dot_x = ws1_rect.right().saturating_sub(1);
    let first_cell =
        &frame.cells[ws1_rect.y as usize * frame.width as usize + first_dot_x as usize];
    let second_cell =
        &frame.cells[ws1_rect.y as usize * frame.width as usize + second_dot_x as usize];
    assert_eq!(first_cell.symbol, unknown_style.0);
    assert_eq!(first_cell.fg, unknown_style.1);
    assert_eq!(second_cell.symbol, working_style.0);
    assert_eq!(second_cell.fg, working_style.1);
}

#[test]
fn folders_mode_drag_onto_group_member_row_assigns_visual_group() {
    let mut state = ClientShellState::new(folders_config());
    let mut projected = folders_snapshot();
    // ws_3 starts ungrouped; dragging it onto ws_2 (a member of "alpha")
    // should join the group rather than reorder.
    projected.workspaces[2].visual_group = None;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("folders layout");

    let source = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_3")
        .expect("ws_3 row")
        .rect;
    let target = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("ws_2 row")
        .rect;

    let down = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: source.x + 1,
        row: source.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(down.actions.is_empty());
    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: target.x + 1,
        row: target.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(drag.repaint);
    assert!(matches!(
        &state.chrome_drag,
        Some(ClientChromeDrag::Workspace {
            source_workspace_id,
            join_group: Some(group),
            ..
        }) if source_workspace_id == "ws_3" && group == "alpha"
    ));

    let release = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: target.x + 1,
        row: target.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceSetGroup(params)
                    if params.workspace_id == "ws_3" && params.group.as_deref() == Some("alpha")
            )
    ));
}

#[test]
fn folders_mode_drag_outside_header_still_reorders() {
    let mut state = ClientShellState::new(folders_config());
    let mut projected = folders_snapshot();
    // Two ungrouped workspaces to reorder among each other, outside any
    // group header or member row.
    projected.workspaces[0].visual_group = None;
    projected.workspaces[1].visual_group = None;
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("folders layout");

    let first = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("ws_1 row")
        .rect;
    let third = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_3")
        .expect("ws_3 row")
        .rect;

    let down = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: first.x + 1,
        row: first.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(down.actions.is_empty());
    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: third.x + 1,
        row: third.bottom(),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(drag.repaint);
    assert!(matches!(
        &state.chrome_drag,
        Some(ClientChromeDrag::Workspace {
            source_workspace_id,
            join_group: None,
            target: Some(_),
        }) if source_workspace_id == "ws_1"
    ));

    let release = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: third.x + 1,
        row: third.bottom(),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceMove(params)
                    if params.workspace_id == "ws_1"
            )
    ));
}

#[test]
fn folders_mode_drag_skips_linked_worktree_row_as_drop_target() {
    let mut state = ClientShellState::new(folders_config());
    let mut projected = snapshot();
    projected.workspaces[0].visual_group = None;
    // Order: ws_1 (drag source), loose_a, ws_linked (linked worktree,
    // ungrouped so it renders as a plain Folders row -- `indented` does
    // not exist there), loose_b, loose_c. Dropping exactly on
    // `ws_linked`'s row must resolve to a real movable-root slot
    // (`workspace_move_method` can never resolve a `before_workspace_id`
    // pointing at a linked worktree), not silently no-op.
    let mut loose_a = projected.workspaces[0].clone();
    loose_a.workspace_id = "loose_a".into();
    loose_a.number = 2;
    loose_a.label = "loose-a".into();
    loose_a.focused = false;
    let mut linked = projected.workspaces[0].clone();
    linked.workspace_id = "ws_linked".into();
    linked.number = 3;
    linked.label = "linked".into();
    linked.focused = false;
    linked.worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    let mut loose_b = projected.workspaces[0].clone();
    loose_b.workspace_id = "loose_b".into();
    loose_b.number = 4;
    loose_b.label = "loose-b".into();
    loose_b.focused = false;
    let mut loose_c = projected.workspaces[0].clone();
    loose_c.workspace_id = "loose_c".into();
    loose_c.number = 5;
    loose_c.label = "loose-c".into();
    loose_c.focused = false;
    projected.workspaces.push(loose_a);
    projected.workspaces.push(linked);
    projected.workspaces.push(loose_b);
    projected.workspaces.push(loose_c);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 24).expect("folders layout");

    let source = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("ws_1 row")
        .rect;
    let linked_rect = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_linked")
        .expect("ws_linked row")
        .rect;

    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: source.x + 1,
        row: source.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: linked_rect.x + 1,
        row: linked_rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &state.chrome_drag,
        Some(ClientChromeDrag::Workspace {
            target: Some((before, _)),
            ..
        }) if before.as_deref() != Some("ws_linked")
    ));

    let release = state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: linked_rect.x + 1,
        row: linked_rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &release.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::WorkspaceMove(params)
                    if params.workspace_id == "ws_1"
            )
    ));
}

#[test]
fn cycle_view_mode_action_cycles_flat_folders_repo_flat() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(folders_snapshot()));
    state.set_pane_surface(surface());
    assert_eq!(state.view_mode, crate::config::ViewMode::Repo);

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CycleViewMode),
        &mut outcome,
    );
    assert_eq!(state.view_mode, crate::config::ViewMode::Flat);
    assert!(outcome.repaint);
    assert!(state.view_mode_manual);

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CycleViewMode),
        &mut outcome,
    );
    assert_eq!(state.view_mode, crate::config::ViewMode::Folders);
    // Regression lock: cycling must actually change what `compose()`
    // renders, not just the `view_mode` field. This is only true if the
    // renderer reads the live `state.view_mode` and not
    // `config.view_mode` (which never moves once cycled).
    state.compose(106, 24).expect("folders render after cycle");
    assert_eq!(state.hits.folders_group_headers.len(), 1);

    let mut outcome = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::CycleViewMode),
        &mut outcome,
    );
    assert_eq!(state.view_mode, crate::config::ViewMode::Repo);
}

/// The Repo path in `render_sidebar` is untouched by the Folders/Flat
/// dispatch above -- it stays the exact `workspace_entries` call it was
/// before. This composes a grouped-worktree snapshot (the one Repo-mode
/// scenario this test suite already characterizes) once under the
/// default `ViewMode::Repo` config and once again explicitly, and checks
/// the rendered frame and hit areas are byte-identical, so a future edit
/// to the Folders/Flat branches cannot silently perturb Repo rendering.
#[test]
fn repo_view_mode_matches_default_rendering() {
    let default_config = Config::default();
    assert_eq!(default_config.ui.view_mode, crate::config::ViewMode::Repo);
    let mut explicit_config = Config::default();
    explicit_config.ui.view_mode = crate::config::ViewMode::Repo;

    let mut projected = snapshot();
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    let mut child = projected.workspaces[0].clone();
    child.workspace_id = "ws_2".into();
    child.number = 2;
    child.label = "repo-feature".into();
    child.focused = false;
    child.worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    projected.workspaces.push(child);

    let mut default_state = ClientShellState::new(ClientShellConfig::from_config(&default_config));
    default_state.set_snapshot(Box::new(projected.clone()));
    default_state.set_pane_surface(surface());
    let default_frame = default_state.compose(106, 24).expect("default repo frame");

    let mut explicit_state =
        ClientShellState::new(ClientShellConfig::from_config(&explicit_config));
    explicit_state.set_snapshot(Box::new(projected));
    explicit_state.set_pane_surface(surface());
    let explicit_frame = explicit_state
        .compose(106, 24)
        .expect("explicit repo frame");

    assert_eq!(default_frame, explicit_frame);
    // Independent signal beyond the two-frame equality above (which alone
    // would also pass if both dispatches were equally broken): assert the
    // Repo-specific content directly. `indented: true` is a hit shape
    // ONLY `workspace_entries`' worktree auto-grouping ever produces --
    // `render_folders_workspace_list` always pushes `indented: false` and
    // never touches `hits.folders_group_headers` is Folders-only, so its
    // absence here proves this render never took the Folders branch.
    assert_eq!(
        default_state.hits.workspaces.len(),
        explicit_state.hits.workspaces.len()
    );
    assert!(default_state.hits.workspaces.iter().any(|hit| hit.indented));
    assert!(explicit_state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.indented));
    assert!(default_state.hits.folders_group_headers.is_empty());
    assert!(explicit_state.hits.folders_group_headers.is_empty());
}
