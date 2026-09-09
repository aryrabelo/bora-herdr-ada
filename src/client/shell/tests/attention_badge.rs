use super::*;

/// The whole text of one composed frame row, so assertions read the badge the
/// way a user sees it instead of poking at render internals.
fn row_text(frame: &crate::protocol::FrameData, y: u16) -> String {
    let start = y as usize * frame.width as usize;
    frame.cells[start..start + frame.width as usize]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect()
}

/// The sidebar header row is the one carrying the " spaces" title; the badge
/// shares it, so locate it by content rather than by re-deriving layout math.
fn header_row(frame: &crate::protocol::FrameData) -> (u16, String) {
    (0..frame.height)
        .map(|y| (y, row_text(frame, y)))
        .find(|(_, text)| text.contains("spaces"))
        .expect("sidebar header row")
}

/// Two panes on the focused workspace, the second one optionally `Blocked`.
/// Both are far past the default 300s idle threshold, so both count as
/// waiting.
fn attention_snapshot(second_pane_status: AgentStatus) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.panes[0].idle_seconds = Some(900);
    projected.panes.push(ClientShellPane {
        pane_id: "pane_2".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        label: None,
        cwd: Some("/repo".into()),
        foreground_cwd: Some("/repo".into()),
        focused: false,
        right_click_passthrough: false,
        idle_seconds: Some(900),
    });
    projected.agents.push(ClientShellAgent {
        pane_id: "pane_2".into(),
        workspace_id: "ws_1".into(),
        tab_id: "tab_1".into(),
        name: None,
        display_agent: None,
        agent: None,
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: second_pane_status,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: false,
    });
    projected
}

fn compose_with(projected: ClientShellSnapshot) -> (ClientShellState, crate::protocol::FrameData) {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 24).expect("sidebar layout");
    (state, frame)
}

#[test]
fn waiting_badge_is_absent_when_no_pane_is_waiting() {
    // Default snapshot: no idle_seconds anywhere, nothing blocked.
    let (_state, frame) = compose_with(snapshot());
    let (_, header) = header_row(&frame);
    assert!(header.contains("spaces"));
    assert!(!header.contains("waiting"), "header was {header:?}");
}

#[test]
fn waiting_badge_counts_idle_panes_in_yellow() {
    let (state, frame) = compose_with(attention_snapshot(AgentStatus::Idle));
    let (y, header) = header_row(&frame);
    let column = header.find("2 waiting").expect("badge text") as u16;
    let cell = &frame.cells[y as usize * frame.width as usize + column as usize];
    assert_eq!(
        cell.fg,
        crate::protocol::color_to_u32(state.config.palette.yellow)
    );
}

#[test]
fn waiting_badge_turns_red_when_any_pane_is_blocked() {
    let (state, frame) = compose_with(attention_snapshot(AgentStatus::Blocked));
    let (y, header) = header_row(&frame);
    let column = header.find("2 waiting").expect("badge text") as u16;
    let cell = &frame.cells[y as usize * frame.width as usize + column as usize];
    assert_eq!(
        cell.fg,
        crate::protocol::color_to_u32(state.config.palette.red)
    );
}

#[test]
fn waiting_badge_ignores_channel_workspace_panes() {
    // Channel workspaces (`#`-prefixed) go quiet by design and carry their
    // own unread badge; they must not inflate this counter.
    let mut projected = attention_snapshot(AgentStatus::Idle);
    projected.workspaces[0].label = "#general".into();
    let (_state, frame) = compose_with(projected);
    let (_, header) = header_row(&frame);
    assert!(!header.contains("waiting"), "header was {header:?}");
}
