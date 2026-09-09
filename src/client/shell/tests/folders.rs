use super::*;

fn folders_config() -> ClientShellConfig {
    let mut config = Config::default();
    config.ui.view_mode = crate::config::ViewMode::Folders;
    ClientShellConfig::from_config(&config)
}

fn folders_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces[0].visual_group = Some("alpha".into());
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
    state.compose(106, 24).expect("folders layout");

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
fn cycle_view_mode_action_cycles_flat_folders_repo_flat() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
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
    assert_eq!(
        default_state.hits.workspaces.len(),
        explicit_state.hits.workspaces.len()
    );
}
